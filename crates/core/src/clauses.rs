//! Speech translated clause by clause: a complete clause is translated once, so each new partial
//! costs only its last, still growing clause, and the final only what changed.

use std::{
    collections::{HashMap, VecDeque},
    sync::Mutex,
};

use crate::{Result, domain::SegmentId, graph::lock, nodes::primary};

/// Segments whose clauses are kept: the one being spoken, and those whose final may still be
/// on its way.
const SEGMENTS_KEPT: usize = 4;

/// Clause translations of the latest segments, shared by their partials and their final.
#[derive(Default)]
pub struct Clauses {
    segments: Mutex<VecDeque<(SegmentId, HashMap<String, String>)>>,
}

impl Clauses {
    /// Translates `text` of `segment`, reusing every run of clauses already translated by an
    /// earlier call, the longest first. Translating happens outside the lock, so callers never
    /// wait for each other.
    ///
    /// Each call to a translator costs a fixed overhead. Unfinished speech still pays it per
    /// missing clause, so the next partial can reuse each one; a final has no next partial, so
    /// its missing clauses that follow each other go in one call.
    pub fn translate(
        &self,
        segment: SegmentId,
        text: &str,
        target: &str,
        is_final: bool,
        mut translate: impl FnMut(&str) -> Result<String>,
    ) -> Result<String> {
        // What to ask the translator for, and whether to keep its answer for later calls.
        enum Part<'a> {
            Known(String),
            Ask(&'a str, bool),
        }
        let clauses = split(text);
        let mut plan = Vec::new();
        let mut i = 0;
        while i < clauses.len() {
            let known = (i..clauses.len()).rev().find_map(|j| {
                let run = span(text, clauses[i], clauses[j]);
                self.get(segment, run).map(|known| (j, known))
            });
            match (known, plan.last_mut()) {
                (Some((j, known)), _) => {
                    plan.push(Part::Known(known));
                    i = j;
                }
                (None, Some(Part::Ask(asked, false))) => *asked = span(text, asked, clauses[i]),
                (None, _) => plan.push(Part::Ask(clauses[i], !is_final)),
            }
            i += 1;
        }
        let parts = plan
            .into_iter()
            .map(|part| match part {
                Part::Known(known) => Ok(known),
                Part::Ask(clause, keep) => {
                    let translated = translate(clause)?;
                    if keep {
                        self.put(segment, clause, &translated);
                    }
                    Ok(translated)
                }
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(parts.join(separator(target)))
    }

    fn get(&self, segment: SegmentId, clause: &str) -> Option<String> {
        let segments = lock(&self.segments);
        let (_, known) = segments.iter().find(|(id, _)| *id == segment)?;
        known.get(&key(clause)).cloned()
    }

    fn put(&self, segment: SegmentId, clause: &str, translated: &str) {
        let mut segments = lock(&self.segments);
        if !segments.iter().any(|(id, _)| *id == segment) {
            if segments.len() == SEGMENTS_KEPT {
                segments.pop_front();
            }
            segments.push_back((segment, HashMap::new()));
        }
        if let Some((_, known)) = segments.iter_mut().find(|(id, _)| *id == segment) {
            known.insert(key(clause), translated.to_owned());
        }
    }
}

/// Clauses end at punctuation followed by a space. The last one may still be growing.
fn split(text: &str) -> Vec<&str> {
    let mut clauses = Vec::new();
    let mut from = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let next_is_space = chars.peek().is_some_and(|(_, n)| n.is_whitespace());
        if matches!(
            c,
            '.' | ',' | '?' | '!' | ';' | ':' | '。' | '、' | '？' | '！'
        ) && next_is_space
        {
            let end = i + c.len_utf8();
            clauses.push(text[from..end].trim());
            from = end;
        }
    }
    clauses.push(text[from..].trim());
    clauses.retain(|c| !c.is_empty());
    clauses
}

/// Unfinished speech often comes without the punctuation and capitals its final gets, so a clause
/// is known by its words alone.
fn key(clause: &str) -> String {
    let words: String = clause
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    words
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The part of `text` from the start of `first` to the end of `last`, both slices of it.
fn span<'a>(text: &'a str, first: &str, last: &str) -> &'a str {
    let offset = |part: &str| part.as_ptr() as usize - text.as_ptr() as usize;
    &text[offset(first)..offset(last) + last.len()]
}

/// Chinese and Japanese are written without spaces between words.
fn separator(target: &str) -> &'static str {
    match primary(target) {
        "zh" | "ja" => "",
        _ => " ",
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[test]
    fn splits_after_punctuation_only() {
        assert_eq!(
            split("Listeners, imagine a friend asks. Would you"),
            ["Listeners,", "imagine a friend asks.", "Would you"]
        );
        assert_eq!(split("3.5 million, so"), ["3.5 million,", "so"]);
        assert_eq!(split("Done."), ["Done."]);
        assert!(split("  ").is_empty());
    }

    #[test]
    fn a_complete_clause_is_translated_once() {
        let asked = RefCell::new(Vec::new());
        let upper = |s: &str| {
            asked.borrow_mut().push(s.to_owned());
            Ok(s.to_uppercase())
        };
        let c = Clauses::default();
        let id = SegmentId::new();
        c.translate(id, "Listeners, imagine", "id", false, upper)
            .unwrap();
        let out = c
            .translate(id, "Listeners, imagine a friend", "id", false, upper)
            .unwrap();
        assert_eq!(out, "LISTENERS, IMAGINE A FRIEND");
        assert_eq!(
            *asked.borrow(),
            ["Listeners,", "imagine", "imagine a friend"]
        );
    }

    #[test]
    fn the_final_reuses_its_partials_and_translates_only_what_changed() {
        let asked = RefCell::new(Vec::new());
        let same = |s: &str| {
            asked.borrow_mut().push(s.to_owned());
            Ok(s.to_owned())
        };
        let c = Clauses::default();
        let id = SegmentId::new();
        c.translate(id, "Hello there, my friend, how", "id", false, same)
            .unwrap();
        asked.borrow_mut().clear();
        let out = c
            .translate(id, "Hello there, my friend, how are you?", "id", true, same)
            .unwrap();
        assert_eq!(out, "Hello there, my friend, how are you?");
        assert_eq!(*asked.borrow(), ["how are you?"]);
    }

    #[test]
    fn missing_clauses_of_a_final_that_follow_each_other_go_in_one_call() {
        let asked = RefCell::new(Vec::new());
        let same = |s: &str| {
            asked.borrow_mut().push(s.to_owned());
            Ok(s.to_owned())
        };
        let c = Clauses::default();
        let id = SegmentId::new();
        c.translate(id, "one, two, three", "id", false, same)
            .unwrap();
        asked.borrow_mut().clear();
        let out = c
            .translate(id, "Zero, one, two, three, four,  five.", "id", true, same)
            .unwrap();
        // "three" was heard without its comma, and is the same clause.
        assert_eq!(out, "Zero, one, two, three four,  five.");
        assert_eq!(*asked.borrow(), ["Zero,", "four,  five."]);
    }

    #[test]
    fn a_final_reuses_unpunctuated_partials_that_span_its_clauses() {
        let asked = RefCell::new(Vec::new());
        let same = |s: &str| {
            asked.borrow_mut().push(s.to_owned());
            Ok(s.to_owned())
        };
        let c = Clauses::default();
        let id = SegmentId::new();
        for partial in [
            "we chose the park because it",
            "we chose the park because it was sunny",
            "we chose the park because it was sunny so",
        ] {
            c.translate(id, partial, "id", false, same).unwrap();
        }
        asked.borrow_mut().clear();
        let out = c
            .translate(
                id,
                "We chose the park, because it was sunny. So nice.",
                "id",
                true,
                same,
            )
            .unwrap();
        assert_eq!(out, "we chose the park because it was sunny So nice.");
        assert_eq!(*asked.borrow(), ["So nice."]);
    }

    #[test]
    fn clauses_of_another_segment_are_not_reused() {
        let asked = RefCell::new(0);
        let count = |s: &str| {
            *asked.borrow_mut() += 1;
            Ok(s.to_owned())
        };
        let c = Clauses::default();
        c.translate(SegmentId::new(), "Hello, friend", "id", false, count)
            .unwrap();
        c.translate(SegmentId::new(), "Hello, friend", "id", false, count)
            .unwrap();
        assert_eq!(*asked.borrow(), 4);
    }

    #[test]
    fn only_the_latest_segments_are_kept() {
        let c = Clauses::default();
        let ids: Vec<_> = (0..=SEGMENTS_KEPT).map(|_| SegmentId::new()).collect();
        for id in &ids {
            c.translate(*id, "Hi.", "id", false, |s| Ok(s.to_owned()))
                .unwrap();
        }
        assert_eq!(c.get(ids[0], "Hi."), None);
        assert_eq!(c.get(ids[SEGMENTS_KEPT], "Hi.").as_deref(), Some("Hi."));
    }

    #[test]
    fn japanese_has_no_spaces_between_clauses() {
        let c = Clauses::default();
        let out = c
            .translate(SegmentId::new(), "Yes, no", "ja", false, |s| {
                Ok(s.to_owned())
            })
            .unwrap();
        assert_eq!(out, "Yes,no");
    }
}
