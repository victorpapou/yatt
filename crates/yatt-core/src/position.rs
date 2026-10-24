//! Fractional indexing for card order.
//!
//! A position is a base-62 fraction written as a string (`"V"` is 0.5), so plain
//! string comparison gives the order. Moving a card computes one new key between
//! its neighbours and never renumbers the rest of the column.

const ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const BASE: u32 = 62;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PositionError {
    #[error("invalid position key {0:?}")]
    InvalidKey(String),
    #[error("lower bound {0:?} is not below upper bound {1:?}")]
    BadOrder(String, String),
}

/// Returns a key strictly between `before` and `after`. `None` means open-ended.
pub fn key_between(before: Option<&str>, after: Option<&str>) -> Result<String, PositionError> {
    let a = before.map(digits).transpose()?.unwrap_or_default();
    let b = after.map(digits).transpose()?;
    if let (Some(lo), Some(hi)) = (before, after)
        && lo >= hi
    {
        return Err(PositionError::BadOrder(lo.into(), hi.into()));
    }
    let mid = midpoint(&a, b.as_deref());
    Ok(mid
        .into_iter()
        .map(|d| ALPHABET[d as usize] as char)
        .collect())
}

fn digits(key: &str) -> Result<Vec<u8>, PositionError> {
    let invalid = || PositionError::InvalidKey(key.into());
    if key.is_empty() || key.ends_with('0') {
        return Err(invalid());
    }
    key.bytes()
        .map(|c| {
            ALPHABET
                .iter()
                .position(|&x| x == c)
                .map(|d| d as u8)
                .ok_or_else(invalid)
        })
        .collect()
}

/// `a < b` as fractions; neither ends in a zero digit. `b == None` means 1.0.
fn midpoint(a: &[u8], b: Option<&[u8]>) -> Vec<u8> {
    if let Some(b) = b {
        let mut n = 0;
        while n < b.len() && a.get(n).copied().unwrap_or(0) == b[n] {
            n += 1;
        }
        if n > 0 {
            let mut out = b[..n].to_vec();
            out.extend(midpoint(a.get(n..).unwrap_or(&[]), Some(&b[n..])));
            return out;
        }
    }
    let da = a.first().copied().unwrap_or(0) as u32;
    let db = b.map_or(BASE, |b| b[0] as u32);
    if db - da > 1 {
        vec![((da + db) / 2) as u8]
    } else if let Some(b) = b.filter(|b| b.len() > 1) {
        vec![b[0]]
    } else {
        let mut out = vec![da as u8];
        out.extend(midpoint(a.get(1..).unwrap_or(&[]), None));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_key_is_middle() {
        assert_eq!(key_between(None, None).unwrap(), "V");
    }

    #[test]
    fn keys_stay_ordered_under_repeated_inserts() {
        let mut keys = vec![key_between(None, None).unwrap()];
        // Deterministic pseudo-random insert points.
        let mut seed = 7u64;
        for _ in 0..2000 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let i = (seed >> 33) as usize % (keys.len() + 1);
            let lo = i.checked_sub(1).map(|j| keys[j].as_str());
            let hi = keys.get(i).map(String::as_str);
            let k = key_between(lo, hi).unwrap();
            if let Some(lo) = lo {
                assert!(lo < k.as_str(), "{lo} < {k}");
            }
            if let Some(hi) = hi {
                assert!(k.as_str() < hi, "{k} < {hi}");
            }
            assert!(!k.ends_with('0'));
            keys.insert(i, k);
        }
    }

    #[test]
    fn tight_neighbours() {
        assert_eq!(key_between(Some("V"), Some("W")).unwrap(), "VV");
        assert_eq!(key_between(Some("V1"), Some("V2")).unwrap(), "V1V");
        let k = key_between(Some("a"), Some("a01")).unwrap();
        assert!("a" < k.as_str() && k.as_str() < "a01");
    }

    #[test]
    fn rejects_bad_input() {
        assert!(key_between(Some("b"), Some("a")).is_err());
        assert!(key_between(Some("a0"), None).is_err());
        assert!(key_between(Some("a-"), None).is_err());
    }
}
