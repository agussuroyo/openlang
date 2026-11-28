use std::collections::HashMap;

use crate::domain::{Caption, SegmentId, Translation};

/// Words shared by two consecutive revisions count as stable. Stability never moves backwards
/// unless the text under it was rewritten.
#[derive(Default)]
pub struct Stabilizer {
    previous: HashMap<SegmentId, Vec<String>>,
}

impl Stabilizer {
    pub fn apply(&mut self, translation: Translation) -> Caption {
        let words: Vec<String> = translation
            .text
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        let id = translation.segment.id;
        let stable_words = if translation.segment.is_final {
            self.previous.remove(&id);
            words.len()
        } else {
            let shared = self.previous.get(&id).map_or(0, |old| {
                old.iter().zip(&words).take_while(|(a, b)| a == b).count()
            });
            self.previous.insert(id, words);
            shared
        };
        Caption {
            translation,
            stable_words,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Origin, Segment};

    fn t(id: SegmentId, text: &str, is_final: bool) -> Translation {
        Translation {
            segment: Segment {
                id,
                revision: 0,
                origin: Origin::System,
                start: Default::default(),
                end: Default::default(),
                is_final,
            },
            source: String::new(),
            language: "id".into(),
            text: text.into(),
        }
    }

    #[test]
    fn only_the_common_prefix_is_stable() {
        let (mut s, id) = (Stabilizer::default(), SegmentId::new());
        assert_eq!(s.apply(t(id, "selamat pagi", false)).stable_words, 0);
        assert_eq!(s.apply(t(id, "selamat pagi semua", false)).stable_words, 2);
        assert_eq!(s.apply(t(id, "selamat siang semua", false)).stable_words, 1);
        assert_eq!(
            s.apply(t(id, "selamat siang semuanya", true)).stable_words,
            3
        );
        assert!(s.previous.is_empty());
    }

    #[test]
    fn segments_do_not_affect_each_other() {
        let mut s = Stabilizer::default();
        s.apply(t(SegmentId::new(), "satu dua", false));
        assert_eq!(
            s.apply(t(SegmentId::new(), "satu dua", false)).stable_words,
            0
        );
    }
}
