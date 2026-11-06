//! Fits ranked items into a token budget. Nothing here tokenizes: the read path has no model to count with, so this estimates, and a caller should leave some margin.
use memex_types::{BudgetReport, Citation, EvidenceItem};

const CHARS_PER_TOKEN: usize = 4;
const ITEM_OVERHEAD: usize = 8;
const CITATION_OVERHEAD: usize = 12;

pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(CHARS_PER_TOKEN)
}

fn citation_cost(c: &Citation) -> usize {
    CITATION_OVERHEAD + c.quote.as_deref().map_or(0, estimate_tokens)
}

/// What an item costs in the answer: its text, citations and conflicts, with room for their fields.
pub fn item_cost(item: &EvidenceItem) -> usize {
    ITEM_OVERHEAD
        + estimate_tokens(&item.text)
        + item.citations.iter().map(citation_cost).sum::<usize>()
        + item.conflicts.iter().map(|c| ITEM_OVERHEAD + estimate_tokens(&c.text)).sum::<usize>()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// The ranks this page holds: `start..end`.
    pub start: usize,
    pub end: usize,
    pub report: BudgetReport,
}

/// Takes ranked items in order until the limit or budget stops it. An item that doesn't fit ends the page, so the next page starts there and the order never changes. A text is never cut, since a shortened quote can't be verified.
pub fn fit(costs: &[usize], start: usize, limit: usize, budget: usize) -> Page {
    let start = start.min(costs.len());
    let (mut end, mut used) = (start, 0);
    let mut truncated = false;
    while end < costs.len() && end - start < limit {
        if used + costs[end] > budget {
            truncated = true;
            break;
        }
        used += costs[end];
        end += 1;
    }
    let smallest_item_needs = (end == start && truncated).then(|| costs[start]);
    Page {
        start,
        end,
        report: BudgetReport {
            limit: budget,
            used,
            truncated,
            smallest_item_needs,
        },
    }
}

/// A cursor is the rank to continue from. It holds nothing else, so it can't expire: a page read after writes reflects the ranking at that moment.
pub fn encode_cursor(rank: usize) -> String {
    format!("r{rank}")
}

pub fn decode_cursor(cursor: &str) -> Option<usize> {
    cursor.strip_prefix('r')?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_count_characters_not_bytes() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        assert_eq!(estimate_tokens("åäöß"), 1);
    }

    #[test]
    fn pages_stop_at_the_budget_or_the_limit() {
        let p = fit(&[10, 10, 10], 0, 2, 100);
        assert_eq!((p.end, p.report.used, p.report.truncated), (2, 20, false));
        let p = fit(&[10, 10, 10], 0, 9, 25);
        assert_eq!((p.end, p.report.used, p.report.truncated), (2, 20, true));
        let p = fit(&[10, 10, 10], 0, 9, 30);
        assert_eq!((p.end, p.report.truncated), (3, false));
    }

    #[test]
    fn an_item_that_does_not_fit_ends_the_page_rather_than_being_skipped() {
        let p = fit(&[10, 500, 10], 0, 9, 100);
        assert_eq!(p.end, 1);
        let p = fit(&[10, 500, 10], 1, 9, 100);
        assert_eq!((p.start, p.end), (1, 1), "the page stays at it");
    }

    #[test]
    fn a_budget_too_small_for_one_item_says_what_it_needs() {
        let p = fit(&[50], 0, 9, 10);
        assert_eq!(p.end, 0);
        assert_eq!(p.report.smallest_item_needs, Some(50));
        assert_eq!(p.report.used, 0);
        assert_eq!(fit(&[50], 0, 9, 50).report.smallest_item_needs, None);
    }

    #[test]
    fn nothing_left_is_not_truncated() {
        let p = fit(&[], 0, 9, 10);
        assert_eq!((p.end, p.report.truncated, p.report.smallest_item_needs), (0, false, None));
        let p = fit(&[5], 7, 9, 10);
        assert_eq!((p.start, p.end, p.report.truncated), (1, 1, false));
    }

    #[test]
    fn pages_cover_everything_in_order_with_no_overlap() {
        let costs: Vec<usize> = (1..40).map(|i| 5 + i % 7).collect();
        let (mut at, mut seen) = (0, vec![]);
        while at < costs.len() {
            let p = fit(&costs, at, 5, 40);
            assert!(p.end > at);
            seen.extend(p.start..p.end);
            at = p.end;
        }
        assert_eq!(seen, (0..costs.len()).collect::<Vec<_>>());
    }

    #[test]
    fn cursors_round_trip_and_bad_ones_are_refused() {
        assert_eq!(decode_cursor(&encode_cursor(42)), Some(42));
        for bad in ["", "r", "rx", "42", "r-1", "r1.5"] {
            assert_eq!(decode_cursor(bad), None, "{bad}");
        }
    }
}
