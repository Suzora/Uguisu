//! One cursor-pagination contract, shared by every list route (ADR 0040).
//!
//! Keyset pagination, not offset: a cursor names the last row of the previous
//! page, so a row inserted while a client walks a list cannot make it see one
//! twice or skip one.
//!
//! Two rules live here because getting them right in five places separately is
//! how they came to disagree. A page is fetched one row longer than it is
//! returned, so a cursor exists *only* when a further row does; and a `limit`
//! outside the allowed range is refused rather than quietly changed, because a
//! client that asked for a page of nothing has a bug that silence hides.

use crate::UguisuError;

/// Page size when a request does not ask for one.
pub const DEFAULT: u32 = 50;

/// Largest page any list route will answer with.
pub const MAX: u32 = 500;

/// The page size to use, or why the request cannot have one.
pub fn limit(requested: Option<u32>, default: u32, max: u32) -> Result<u32, UguisuError> {
    match requested {
        None => Ok(default),
        Some(n) if n >= 1 && n <= max => Ok(n),
        Some(n) => Err(UguisuError::Invalid(format!(
            "`limit` is between 1 and {max}, not {n}"
        ))),
    }
}

/// Cuts an over-fetched page down to `limit` and reports the next cursor.
///
/// `rows` must have been read with `limit + 1` as its SQL limit. The extra row
/// is the whole mechanism: its presence is what "there is more" means, and
/// without it a page that happens to be exactly full reports a cursor that
/// leads to nothing.
pub fn truncate<T, I>(
    mut rows: Vec<T>,
    limit: u32,
    id_of: impl Fn(&T) -> I,
) -> (Vec<T>, Option<I>) {
    let limit = limit as usize;
    if rows.len() <= limit {
        return (rows, None);
    }
    rows.truncate(limit);
    let next = rows.last().map(id_of);
    (rows, next)
}

/// How many rows to read for a page of `limit`.
#[must_use]
pub fn over_fetch(limit: u32) -> u32 {
    limit.saturating_add(1)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn a_limit_outside_the_range_is_refused() {
        assert_eq!(limit(None, 50, 500).ok(), Some(50));
        assert_eq!(limit(Some(1), 50, 500).ok(), Some(1));
        assert_eq!(limit(Some(500), 50, 500).ok(), Some(500));
        for refused in [0, 501, u32::MAX] {
            let err = limit(Some(refused), 50, 500).unwrap_err();
            assert!(matches!(err, UguisuError::Invalid(_)), "{refused}: {err}");
            assert!(err.to_string().contains("between 1 and 500"), "{err}");
        }
    }

    #[test]
    fn a_full_page_with_nothing_after_it_has_no_cursor() {
        // Three rows read for a page of two: there is more.
        let (page, next) = truncate(vec![1, 2, 3], 2, |n| *n);
        assert_eq!(page, vec![1, 2]);
        assert_eq!(next, Some(2));

        // Two rows read for a page of two: this is the end, even though the
        // page is exactly full. That is the case the old code got wrong.
        let (page, next) = truncate(vec![1, 2], 2, |n| *n);
        assert_eq!(page, vec![1, 2]);
        assert_eq!(next, None);

        let (page, next) = truncate(vec![1], 2, |n| *n);
        assert_eq!(page, vec![1]);
        assert_eq!(next, None);

        let (page, next) = truncate(Vec::<i32>::new(), 2, |n| *n);
        assert!(page.is_empty());
        assert_eq!(next, None);
    }

    #[test]
    fn over_fetch_reads_one_more() {
        assert_eq!(over_fetch(1), 2);
        assert_eq!(over_fetch(500), 501);
        assert_eq!(over_fetch(u32::MAX), u32::MAX, "and never wraps");
    }
}
