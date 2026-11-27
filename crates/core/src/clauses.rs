//! Unfinished speech translated clause by clause: a clause that is complete is translated once,
//! so each new partial costs only its last, still growing clause.

use crate::{Result, domain::SegmentId, nodes::primary};

/// Clause translations of the segment being spoken.
#[derive(Default)]
pub struct Clauses {
    segment: Option<SegmentId>,
    /// Complete clauses and their translations, in order.
    done: Vec<(String, String)>,
}

impl Clauses {
    /// Translates `text` of `segment` with `translate`, reusing the complete clauses it already
    /// translated. A clause whose words changed is translated again.
    pub fn translate(
        &mut self,
        segment: SegmentId,
        text: &str,
        target: &str,
        mut translate: impl FnMut(&str) -> Result<String>,
    ) -> Result<String> {
        if self.segment != Some(segment) {
            self.segment = Some(segment);
            self.done.clear();
        }
        let mut clauses = split(text);
        let tail = clauses.pop().unwrap_or_default();
        let kept = self
            .done
            .iter()
            .zip(&clauses)
            .take_while(|((source, _), clause)| source == *clause)
            .count();
        self.done.truncate(kept);
        for clause in &clauses[kept..] {
            let translated = translate(clause)?;
            self.done.push(((*clause).to_owned(), translated));
        }
        let mut parts: Vec<String> = self.done.iter().map(|(_, t)| t.clone()).collect();
        if !tail.is_empty() {
            parts.push(translate(tail)?);
        }
        Ok(parts.join(separator(target)))
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
        let mut c = Clauses::default();
        let id = SegmentId::new();
        c.translate(id, "Listeners, imagine", "id", upper).unwrap();
        let out = c
            .translate(id, "Listeners, imagine a friend", "id", upper)
            .unwrap();
        assert_eq!(out, "LISTENERS, IMAGINE A FRIEND");
        assert_eq!(
            *asked.borrow(),
            ["Listeners,", "imagine", "imagine a friend"]
        );
    }

    #[test]
    fn a_rewritten_clause_and_a_new_segment_start_again() {
        let asked = RefCell::new(0);
        let count = |s: &str| {
            *asked.borrow_mut() += 1;
            Ok(s.to_owned())
        };
        let mut c = Clauses::default();
        let id = SegmentId::new();
        c.translate(id, "Hello there, friend", "id", count).unwrap();
        c.translate(id, "Hello, there, friend", "id", count)
            .unwrap();
        // "Hello there," became "Hello," and "there,": both new, plus the tail.
        assert_eq!(*asked.borrow(), 2 + 3);
        c.translate(SegmentId::new(), "Hello, there, friend", "id", count)
            .unwrap();
        assert_eq!(*asked.borrow(), 5 + 3);
    }

    #[test]
    fn japanese_has_no_spaces_between_clauses() {
        let mut c = Clauses::default();
        let out = c
            .translate(SegmentId::new(), "Yes, no", "ja", |s| Ok(s.to_owned()))
            .unwrap();
        assert_eq!(out, "Yes,no");
    }
}
