//! An amount of rupees in English words, as an invoice prints its total.

const ONES: [&str; 20] = [
    "Zero",
    "One",
    "Two",
    "Three",
    "Four",
    "Five",
    "Six",
    "Seven",
    "Eight",
    "Nine",
    "Ten",
    "Eleven",
    "Twelve",
    "Thirteen",
    "Fourteen",
    "Fifteen",
    "Sixteen",
    "Seventeen",
    "Eighteen",
    "Nineteen",
];

const TENS: [&str; 10] = ["", "", "Twenty", "Thirty", "Forty", "Fifty", "Sixty", "Seventy", "Eighty", "Ninety"];

/// The international scale, smallest first: each step is a thousand times the last.
const SCALES: [&str; 7] = ["", "Thousand", "Million", "Billion", "Trillion", "Quadrillion", "Quintillion"];

/// `rupees` in words, in the international system without commas or "and": 13,425 is "Thirteen Thousand Four
/// Hundred Twenty-Five Rupees Only", 0 is "Zero Rupees Only" and 1 is "One Rupee Only". A negative amount (a credit
/// note) starts "Minus ".
pub fn lkr_in_words(rupees: i64) -> String {
    let magnitude = rupees.unsigned_abs();
    let words = if magnitude == 0 {
        ONES[0].to_owned()
    } else {
        let mut groups = Vec::new();
        let mut rest = magnitude;
        let mut scale = 0;
        while rest > 0 {
            let group = (rest % 1000) as usize;
            if group > 0 {
                let mut words = below_thousand(group);
                if !SCALES[scale].is_empty() {
                    words.push(' ');
                    words.push_str(SCALES[scale]);
                }
                groups.push(words);
            }
            rest /= 1000;
            scale += 1;
        }
        groups.reverse();
        groups.join(" ")
    };
    let sign = if rupees < 0 { "Minus " } else { "" };
    let unit = if magnitude == 1 { "Rupee" } else { "Rupees" };
    format!("{sign}{words} {unit} Only")
}

/// 1 to 999 in words.
fn below_thousand(n: usize) -> String {
    let mut parts = Vec::new();
    if n >= 100 {
        parts.push(format!("{} Hundred", ONES[n / 100]));
    }
    let rest = n % 100;
    if rest >= 20 {
        let tens = TENS[rest / 10];
        parts.push(if rest.is_multiple_of(10) { tens.to_owned() } else { format!("{tens}-{}", ONES[rest % 10]) });
    } else if rest > 0 {
        parts.push(ONES[rest].to_owned());
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::lkr_in_words;

    #[test]
    fn amounts_read_in_the_international_system() {
        for (rupees, words) in [
            (0, "Zero Rupees Only"),
            (1, "One Rupee Only"),
            (21, "Twenty-One Rupees Only"),
            (100, "One Hundred Rupees Only"),
            (115, "One Hundred Fifteen Rupees Only"),
            (13_425, "Thirteen Thousand Four Hundred Twenty-Five Rupees Only"),
            (100_000, "One Hundred Thousand Rupees Only"),
            (10_000_000, "Ten Million Rupees Only"),
            (1_000_000_000, "One Billion Rupees Only"),
            (1_000_001, "One Million One Rupees Only"),
            (
                999_999_999_999,
                "Nine Hundred Ninety-Nine Billion Nine Hundred Ninety-Nine Million Nine Hundred Ninety-Nine Thousand \
                 Nine Hundred Ninety-Nine Rupees Only",
            ),
        ] {
            assert_eq!(lkr_in_words(rupees), words, "{rupees}");
        }
    }

    #[test]
    fn a_negative_amount_is_minus() {
        assert_eq!(lkr_in_words(-13_425), "Minus Thirteen Thousand Four Hundred Twenty-Five Rupees Only");
        assert_eq!(lkr_in_words(-1), "Minus One Rupee Only");
        assert!(lkr_in_words(i64::MIN).starts_with("Minus Nine Quintillion"));
    }
}
