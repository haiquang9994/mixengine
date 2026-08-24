//! A tag-length-value reader, enough to refuse a certificate and no more.
//!
//! **Hand-written, and the alternative was measured** — the T49a design, D11. `x509-parser` is 29
//! crates with 7 already present, so 22 net into `mixengine-elevate`, a binary that runs as root and
//! whose dependency closure CI diffs against a checked-in list. What is needed here is not a parser
//! that *understands* a certificate but a reader that walks to a handful of places and refuses
//! everything else, and that is small enough to be this file.
//!
//! **Nothing here may panic.** A panic in the helper leaves no response file, and the response file
//! is the protocol — a daemon cannot tell that from a helper that never started. So every read is a
//! `get`, there is no indexing anywhere in this module, and there are tests that feed it every
//! prefix of a valid encoding and every single byte.
//!
//! **Strict about encodings on purpose.** Indefinite lengths, non-minimal lengths and high tag
//! numbers are refused rather than handled. None of them appears in a certificate `rcgen` produces,
//! and a reader that accepted two encodings of one value would accept two byte strings for one
//! certificate — which is exactly what a check whose job is to keep a set enumerable must not do.

/// Why some bytes are not the certificate they claimed to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Malformed(pub(crate) String);

impl std::fmt::Display for Malformed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A refusal with a fixed sentence.
fn malformed(what: &str) -> Malformed {
    Malformed(what.to_owned())
}

pub(crate) const INTEGER: u8 = 0x02;
pub(crate) const OCTET_STRING: u8 = 0x04;
pub(crate) const OID: u8 = 0x06;
pub(crate) const UTF8_STRING: u8 = 0x0c;
pub(crate) const PRINTABLE_STRING: u8 = 0x13;
pub(crate) const UTC_TIME: u8 = 0x17;
pub(crate) const GENERALIZED_TIME: u8 = 0x18;
pub(crate) const SEQUENCE: u8 = 0x30;
pub(crate) const SET: u8 = 0x31;

/// `[0] EXPLICIT`, which is where a certificate keeps its version.
pub(crate) const CONTEXT_0: u8 = 0xa0;

/// `[3] EXPLICIT`, which is where a certificate keeps its extensions.
pub(crate) const CONTEXT_3: u8 = 0xa3;

/// One element: what it is, what is inside it, and its own bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Element<'a> {
    /// The identifier octet.
    pub(crate) tag: u8,

    /// Between the length and the next element.
    pub(crate) contents: &'a [u8],

    /// Tag, length and contents together.
    ///
    /// **This is what an equality check compares**, and it is why the type carries it rather than
    /// leaving a caller to reconstruct it: two names are the same name when these bytes match, which
    /// is a comparison that needs no name parsing at all.
    pub(crate) raw: &'a [u8],
}

impl Element<'_> {
    /// The contents of this element, having first insisted it is a `tag`.
    ///
    /// `what` names the field for the refusal, because "expected 0x30" is a sentence nobody reading
    /// an audit log a week later can do anything with.
    pub(crate) fn expect(&self, tag: u8, what: &str) -> Result<&[u8], Malformed> {
        if self.tag == tag {
            Ok(self.contents)
        } else {
            Err(Malformed(format!(
                "{what} is tagged {:#04x} rather than {tag:#04x}",
                self.tag
            )))
        }
    }
}

/// Read one element from the front of `input`, and hand back whatever follows it.
pub(crate) fn read(input: &[u8]) -> Result<(Element<'_>, &[u8]), Malformed> {
    let tag = *input.first().ok_or_else(|| malformed("no tag"))?;

    // High-tag-number form. Nothing in a certificate needs it, so it is refused rather than read.
    if tag & 0x1f == 0x1f {
        return Err(malformed("a tag in high-tag-number form"));
    }

    let first = *input.get(1).ok_or_else(|| malformed("no length"))?;

    let (length, header) = if first < 0x80 {
        (usize::from(first), 2)
    } else if first == 0x80 {
        return Err(malformed("an indefinite length, which is BER and not DER"));
    } else {
        let count = usize::from(first & 0x7f);

        // Four bytes is 4 GiB, and `MAX_DER` refuses anything over a few kilobytes long before
        // this. A wider length is a document nothing in this project produces.
        if count > 4 {
            return Err(malformed("a length wider than this reader will read"));
        }

        let bytes = input
            .get(2..2 + count)
            .ok_or_else(|| malformed("a length that runs off the end"))?;

        if bytes.first() == Some(&0) {
            return Err(malformed(
                "a length with a leading zero, which is not minimal",
            ));
        }

        let mut value: usize = 0;
        for byte in bytes {
            value = (value << 8) | usize::from(*byte);
        }

        if value < 0x80 {
            return Err(malformed("a long-form length that fits in short form"));
        }

        (value, 2 + count)
    };

    let end = header
        .checked_add(length)
        .ok_or_else(|| malformed("a length that overflows"))?;

    let raw = input
        .get(..end)
        .ok_or_else(|| malformed("an element that runs off the end"))?;
    let contents = raw
        .get(header..)
        .ok_or_else(|| malformed("an element shorter than its own header"))?;
    let rest = input
        .get(end..)
        .ok_or_else(|| malformed("an element that runs off the end"))?;

    Ok((Element { tag, contents, raw }, rest))
}

/// Read one element that is the whole of `input`.
///
/// A certificate is one element and nothing after it. Trailing bytes are a second document sharing a
/// file with the first, and accepting them would mean the bytes a check ran over are not the bytes
/// that get installed.
pub(crate) fn only(input: &[u8]) -> Result<Element<'_>, Malformed> {
    let (element, rest) = read(input)?;

    if rest.is_empty() {
        Ok(element)
    } else {
        Err(malformed(
            "bytes after the element that should have been the last",
        ))
    }
}

/// Every element inside a constructed element's contents.
pub(crate) fn children(contents: &[u8]) -> Result<Vec<Element<'_>>, Malformed> {
    let mut rest = contents;
    let mut found = Vec::new();

    while !rest.is_empty() {
        let (element, after) = read(rest)?;
        found.push(element);
        rest = after;
    }

    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape everything else is built on: a SEQUENCE holding one INTEGER, and a byte after it.
    #[test]
    fn a_sequence_hands_back_its_contents_and_what_follows() {
        let bytes = [0x30, 0x03, 0x02, 0x01, 0x07, 0xff];

        let (element, rest) = read(&bytes).expect("a sequence");

        assert_eq!(element.tag, SEQUENCE);
        assert_eq!(element.contents, &[0x02, 0x01, 0x07]);
        assert_eq!(element.raw, &[0x30, 0x03, 0x02, 0x01, 0x07]);
        assert_eq!(rest, &[0xff]);
    }

    /// A length that needs two bytes, which every real certificate has.
    #[test]
    fn a_long_form_length_is_read() {
        let mut bytes = vec![0x04, 0x82, 0x01, 0x00];
        bytes.extend(std::iter::repeat_n(0xaa, 256));

        let (element, rest) = read(&bytes).expect("an octet string");

        assert_eq!(element.contents.len(), 256);
        assert!(rest.is_empty());
    }

    /// **BER, not DER.** Refused rather than followed: this reader exists to say no.
    #[test]
    fn an_indefinite_length_is_refused() {
        assert!(read(&[0x30, 0x80, 0x00, 0x00]).is_err());
    }

    /// A length that could have been written shorter is a second encoding of one value, and a
    /// reader that accepted both would accept two byte strings for one certificate.
    #[test]
    fn a_length_that_is_not_minimal_is_refused() {
        assert!(
            read(&[0x04, 0x81, 0x01, 0xaa]).is_err(),
            "0x81 0x01 is 1, which fits in short form"
        );
        assert!(
            read(&[0x04, 0x82, 0x00, 0x81]).is_err(),
            "a leading zero in the length"
        );
    }

    /// Nothing in a certificate uses it, so it is refused rather than implemented.
    #[test]
    fn a_high_tag_number_is_refused() {
        assert!(read(&[0x1f, 0x81, 0x00, 0x00]).is_err());
    }

    /// **The property this whole file is written for.** A panic in `mixengine-elevate` means no
    /// response file at all, which a daemon cannot tell from a helper that never ran.
    #[test]
    fn no_prefix_of_a_valid_encoding_can_make_this_panic() {
        let valid = [0x30, 0x06, 0x02, 0x01, 0x07, 0x04, 0x01, 0xaa];

        for cut in 0..=valid.len() {
            let truncated = valid.get(..cut).expect("a prefix of its own length");

            // The assertion is that each of them *answers*. Whether it says yes or no is the
            // business of the check next door, not of the reader.
            let _ = read(truncated);
            let _ = only(truncated);
            let _ = children(truncated);
        }
    }

    /// Every byte, not only the well-formed ones.
    #[test]
    fn no_single_byte_can_make_this_panic() {
        for tag in 0..=u8::MAX {
            let _ = read(&[tag]);
            let _ = read(&[tag, 0xff]);
            let _ = read(&[SEQUENCE, tag]);
            let _ = read(&[tag, 0x82, 0xff, 0xff]);
            let _ = children(&[tag, 0x01, 0x00]);
        }
    }

    /// A length that would overflow the pointer width is arithmetic, not a read.
    #[test]
    fn a_length_that_overflows_is_refused() {
        assert!(read(&[0x04, 0x84, 0xff, 0xff, 0xff, 0xff]).is_err());
    }

    /// `only` is `read` plus "and nothing after it", which is what a certificate is.
    #[test]
    fn trailing_bytes_after_the_one_element_are_refused() {
        assert!(only(&[0x02, 0x01, 0x07]).is_ok());
        assert!(only(&[0x02, 0x01, 0x07, 0x00]).is_err());
    }

    #[test]
    fn children_reads_every_element_of_a_sequences_contents() {
        let inside = [0x02, 0x01, 0x07, 0x04, 0x01, 0xaa];

        let found = children(&inside).expect("two elements");

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].tag, INTEGER);
        assert_eq!(found[1].tag, OCTET_STRING);
    }

    /// An empty constructed element is empty, not an error: a `Name` with no components is a
    /// document to refuse next door, on a rule about certificates rather than about encodings.
    #[test]
    fn children_of_nothing_is_nothing() {
        assert_eq!(children(&[]).expect("no elements").len(), 0);
    }

    /// The refusal names the field, because an audit log read a week later is the only reader.
    #[test]
    fn expecting_the_wrong_tag_says_which_field_it_was() {
        let (element, _) = read(&[0x02, 0x01, 0x07]).expect("an integer");

        let refused = element.expect(SEQUENCE, "the certificate").unwrap_err();

        assert!(refused.to_string().contains("the certificate"), "{refused}");
    }
}
