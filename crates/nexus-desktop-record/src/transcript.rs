//! The transcriber's words (src/media.rs: each with its start and length) grouped into phrases: a new phrase
//! at a pause, after a sentence once it has a few words, or once one runs long. Pure.

use super::model::Phrase;

#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub text: String,
    pub start: f64,
    pub duration: f64,
}

/// A silence this long ends a phrase (seconds).
const PAUSE: f64 = 0.7;
/// A sentence's end ends a phrase that has at least this many words.
const SENTENCE: usize = 3;
/// No phrase runs longer than this many words.
const LONGEST: usize = 30;

pub fn phrases(words: &[Word]) -> Vec<Phrase> {
    let mut out: Vec<Phrase> = Vec::new();
    let mut count = 0;
    let mut open = false;
    for word in words {
        let text = word.text.trim();
        if text.is_empty() {
            continue;
        }
        let end = word.start + word.duration.max(0.0);
        match out.last_mut() {
            Some(phrase) if open && word.start - phrase.end < PAUSE && count < LONGEST => {
                phrase.text.push(' ');
                phrase.text.push_str(text);
                phrase.end = end;
                count += 1;
            }
            _ => {
                out.push(Phrase {
                    start: word.start,
                    end,
                    text: text.to_string(),
                });
                count = 1;
            }
        }
        open = !(count >= SENTENCE && text.ends_with(['.', '?', '!']));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, start: f64) -> Word {
        Word {
            text: text.into(),
            start,
            duration: 0.3,
        }
    }

    #[test]
    fn pauses_and_sentences_split_phrases() {
        let words = [
            word("So", 0.0),
            word("this", 0.4),
            word("page.", 0.8),
            word("This", 1.2),
            word("button", 1.6),
            word("", 1.8),
            word("overflows", 2.0),
            word("here", 4.0),
            word("Ok.", 4.4),
        ];
        let phrases = phrases(&words);
        let texts: Vec<&str> = phrases.iter().map(|phrase| phrase.text.as_str()).collect();
        assert_eq!(texts, vec!["So this page.", "This button overflows", "here Ok."]);
        assert_eq!((phrases[1].start, phrases[1].end), (1.2, 2.3));
    }

    #[test]
    fn a_short_sentence_runs_on() {
        let phrases = phrases(&[word("Yes.", 0.0), word("That", 0.4), word("one.", 0.8)]);
        assert_eq!(phrases.len(), 1);
        assert_eq!(phrases[0].text, "Yes. That one.");
    }
}
