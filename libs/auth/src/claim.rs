use std::fmt::Display;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub uid: usize,
    pub exp: usize,
    pub mode: Mode,
}

impl Display for Claims {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "uid: {}\nexpire: {}, mode: {}",
            self.uid,
            self.exp,
            Access::from(self.mode)
        )
    }
}

/// Look like Linux File-system permissions
/// 4: read, allow use whep subscribe
/// 2: write, allow use whip publish
/// 1: execute, allow use manager this, example: destroy
type Mode = u8;

impl From<Mode> for Access {
    fn from(mask: Mode) -> Access {
        Access {
            r: mask & 4 != 0,
            w: mask & 2 != 0,
            x: mask & 1 != 0,
        }
    }
}

struct Access {
    r: bool,
    w: bool,
    x: bool,
}

impl Display for Access {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}{}{}",
            if self.r { "r" } else { "-" },
            if self.w { "w" } else { "-" },
            if self.x { "x" } else { "-" },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mode() {
        let mut access = Access::from(7);
        assert!(access.r);
        assert!(access.w);
        assert!(access.x);
        assert_eq!(format!("{}", access), "rwx");

        access = Access::from(6);
        assert!(access.r);
        assert!(access.w);
        assert!(!access.x);
        assert_eq!(format!("{}", access), "rw-");

        access = Access::from(5);
        assert!(access.r);
        assert!(!access.w);
        assert!(access.x);
        assert_eq!(format!("{}", access), "r-x");

        access = Access::from(4);
        assert!(access.r);
        assert!(!access.w);
        assert!(!access.x);
        assert_eq!(format!("{}", access), "r--");

        access = Access::from(1);
        assert!(!access.r);
        assert!(!access.w);
        assert!(access.x);
        assert_eq!(format!("{}", access), "--x");

        access = Access::from(0);
        assert!(!access.r);
        assert!(!access.w);
        assert!(!access.x);
        assert_eq!(format!("{}", access), "---");
    }
}
