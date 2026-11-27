//! Typed processing graph. Every node runs on its own thread behind its own queue, so a slow or
//! broken branch cannot hold back the others.

use std::{
    collections::VecDeque,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use crate::{
    Error, Result,
    domain::{Packet, PacketKind},
};

pub trait Node: Send {
    fn name(&self) -> String;
    fn accepts(&self) -> &'static [PacketKind];
    fn emits(&self) -> &'static [PacketKind];

    fn queue(&self) -> QueuePolicy {
        QueuePolicy::Block(256)
    }

    /// The emitter may be cloned and used from any thread until `finish` returns.
    fn start(&mut self, _out: Emitter) -> Result<()> {
        Ok(())
    }

    /// An error stops this node only. Report recoverable problems with `Emitter::report`.
    fn handle(&mut self, packet: Packet) -> Result<()>;

    /// Must return only after every remaining output has been emitted.
    fn finish(&mut self) -> Result<()> {
        Ok(())
    }

    /// Partial packets are held back, and replaced by newer ones, until this moment.
    fn ready_at(&self) -> Option<Instant> {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueuePolicy {
    /// Slows the producer down when full. Nothing is lost.
    Block(usize),
    /// A newer partial replaces the queued partial of the same segment. Finals are never lost.
    Coalesce,
}

pub type Reporter = Arc<dyn Fn(&str, Error) + Send + Sync>;

struct QueueState {
    items: VecDeque<Packet>,
    open_inputs: usize,
}

struct Queue {
    policy: QueuePolicy,
    state: Mutex<QueueState>,
    changed: Condvar,
    cancelled: Arc<AtomicBool>,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Queue {
    fn push(&self, packet: Packet) {
        let mut s = lock(&self.state);
        match self.policy {
            QueuePolicy::Block(capacity) => {
                while s.items.len() >= capacity && !self.cancelled.load(SeqCst) {
                    s = self.changed.wait(s).unwrap_or_else(PoisonError::into_inner);
                }
            }
            QueuePolicy::Coalesce => {
                let id = packet.segment().map(|x| x.id);
                s.items.retain(|old| {
                    !(old.kind() == packet.kind()
                        && old.is_partial()
                        && old.segment().map(|x| x.id) == id)
                });
            }
        }
        if !self.cancelled.load(SeqCst) {
            s.items.push_back(packet);
        }
        self.changed.notify_all();
    }

    fn close_input(&self) {
        lock(&self.state).open_inputs -= 1;
        self.changed.notify_all();
    }

    fn pop(&self, ready_at: Option<Instant>) -> Option<Packet> {
        let mut s = lock(&self.state);
        loop {
            if self.cancelled.load(SeqCst) {
                return None;
            }
            let wait = match s.items.front() {
                Some(front) if front.is_partial() && s.open_inputs > 0 => {
                    ready_at.and_then(|at| at.checked_duration_since(Instant::now()))
                }
                Some(_) => None,
                None if s.open_inputs == 0 => return None,
                None => {
                    s = self.changed.wait(s).unwrap_or_else(PoisonError::into_inner);
                    continue;
                }
            };
            match wait.filter(|d| !d.is_zero()) {
                Some(d) => {
                    s = self
                        .changed
                        .wait_timeout(s, d)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
                None => {
                    let packet = s.items.pop_front();
                    self.changed.notify_all();
                    return packet;
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct Emitter {
    name: Arc<str>,
    targets: Arc<[(&'static [PacketKind], Arc<Queue>)]>,
    reporter: Reporter,
}

impl Emitter {
    pub fn emit(&self, packet: Packet) {
        for (accepts, queue) in self.targets.iter() {
            if accepts.contains(&packet.kind()) {
                queue.push(packet.clone());
            }
        }
    }

    pub fn report(&self, error: Error) {
        (self.reporter)(&self.name, error);
    }

    fn close(&self) {
        for (_, queue) in self.targets.iter() {
            queue.close_input();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeId(usize);

struct Slot {
    node: Box<dyn Node>,
    targets: Vec<usize>,
    inputs: usize,
}

#[derive(Default)]
pub struct GraphBuilder {
    slots: Vec<Slot>,
}

impl GraphBuilder {
    /// Adds a node fed by `from`. Because inputs must already exist, the graph cannot have cycles.
    pub fn add(&mut self, node: Box<dyn Node>, from: &[NodeId]) -> Result<NodeId> {
        if from.is_empty() {
            return Err(Error::Graph(format!("{} has no input", node.name())));
        }
        for id in from {
            let up = &self.slots[id.0].node;
            if !up.emits().iter().any(|k| node.accepts().contains(k)) {
                return Err(Error::Graph(format!(
                    "{} emits {:?} but {} accepts {:?}",
                    up.name(),
                    up.emits(),
                    node.name(),
                    node.accepts()
                )));
            }
        }
        let id = self.slots.len();
        for f in from {
            self.slots[f.0].targets.push(id);
        }
        self.slots.push(Slot {
            node,
            targets: vec![],
            inputs: from.len(),
        });
        Ok(NodeId(id))
    }

    /// Adds the node that receives packets from `Graph::input`.
    pub fn root(&mut self, node: Box<dyn Node>) -> Result<NodeId> {
        if !self.slots.is_empty() {
            return Err(Error::Graph("root must be added first".into()));
        }
        self.slots.push(Slot {
            node,
            targets: vec![],
            inputs: 1,
        });
        Ok(NodeId(0))
    }

    /// Starts every node. If one fails to start, nothing keeps running.
    pub fn start(mut self, reporter: Reporter) -> Result<Graph> {
        if self.slots.is_empty() {
            return Err(Error::Graph("empty graph".into()));
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let queues: Vec<Arc<Queue>> = self
            .slots
            .iter()
            .map(|s| {
                Arc::new(Queue {
                    policy: s.node.queue(),
                    state: Mutex::new(QueueState {
                        items: VecDeque::new(),
                        open_inputs: s.inputs,
                    }),
                    changed: Condvar::new(),
                    cancelled: cancelled.clone(),
                })
            })
            .collect();
        let accepts: Vec<_> = self.slots.iter().map(|s| s.node.accepts()).collect();
        let emitter = |name: String, targets: &[usize]| Emitter {
            name: name.into(),
            targets: targets
                .iter()
                .map(|&t| (accepts[t], queues[t].clone()))
                .collect(),
            reporter: reporter.clone(),
        };
        let input = emitter("input".into(), &[0]);

        let mut emitters = Vec::new();
        for slot in &mut self.slots {
            let out = emitter(slot.node.name(), &slot.targets);
            if let Err(e) = slot.node.start(out.clone()) {
                cancelled.store(true, SeqCst);
                return Err(e);
            }
            emitters.push(out);
        }

        let failed = Arc::new(AtomicUsize::new(0));
        let threads = self
            .slots
            .into_iter()
            .zip(emitters)
            .zip(&queues)
            .map(|((slot, out), queue)| {
                let (queue, failed) = (queue.clone(), failed.clone());
                std::thread::spawn(move || run(slot.node, out, &queue, &failed))
            })
            .collect();
        Ok(Graph {
            input,
            queues,
            threads,
            cancelled,
            failed,
            closed: false,
        })
    }
}

fn run(mut node: Box<dyn Node>, out: Emitter, queue: &Queue, failed: &AtomicUsize) {
    let mut alive = true;
    let guarded = |node: &mut Box<dyn Node>, f: &mut dyn FnMut(&mut dyn Node) -> Result<()>| {
        let result = catch_unwind(AssertUnwindSafe(|| f(node.as_mut())))
            .unwrap_or_else(|_| Err(Error::Graph("node panicked".into())));
        if let Err(e) = result {
            failed.fetch_add(1, SeqCst);
            out.report(e);
            return false;
        }
        true
    };
    while let Some(packet) = queue.pop(if alive { node.ready_at() } else { None }) {
        if alive {
            let mut packet = Some(packet);
            alive = guarded(&mut node, &mut |n| match packet.take() {
                Some(p) => n.handle(p),
                None => Ok(()),
            });
        }
    }
    guarded(&mut node, &mut |n| n.finish());
    drop(node);
    out.close();
}

pub struct Graph {
    input: Emitter,
    queues: Vec<Arc<Queue>>,
    threads: Vec<JoinHandle<()>>,
    cancelled: Arc<AtomicBool>,
    failed: Arc<AtomicUsize>,
    closed: bool,
}

impl Graph {
    pub fn input(&self) -> Emitter {
        self.input.clone()
    }

    pub fn failures(&self) -> usize {
        self.failed.load(SeqCst)
    }

    /// Lets every queued packet and every final segment through, then stops.
    pub fn finish(mut self) {
        self.close();
    }

    /// Like `finish`, but a node that does not return in time is left behind and its output
    /// discarded. Returns false when that happened.
    pub fn finish_within(mut self, limit: Duration) -> bool {
        self.closed = true;
        self.input.close();
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline && !self.threads.iter().all(JoinHandle::is_finished) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let clean = self.threads.iter().all(JoinHandle::is_finished);
        if !clean {
            self.wake_cancelled();
            self.threads.retain(JoinHandle::is_finished);
        }
        self.close();
        clean
    }

    fn wake_cancelled(&self) {
        self.cancelled.store(true, SeqCst);
        for q in &self.queues {
            // Taking the lock first means a waiter cannot miss the wake-up.
            drop(lock(&q.state));
            q.changed.notify_all();
        }
    }

    /// Stops as soon as the running calls return. Queued packets are discarded.
    pub fn cancel(mut self) {
        self.wake_cancelled();
        self.close();
    }

    fn close(&mut self) {
        if !std::mem::replace(&mut self.closed, true) {
            self.input.close();
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

impl Drop for Graph {
    fn drop(&mut self) {
        self.close();
    }
}
