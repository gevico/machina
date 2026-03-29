/// Address newtype wrappers for guest/host address spaces.
use std::fmt;

macro_rules! addr_type {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq,
            PartialOrd, Ord, Hash,
        )]
        #[repr(transparent)]
        pub struct $name(pub u64);

        impl $name {
            #[inline]
            pub fn new(addr: u64) -> Self {
                Self(addr)
            }

            #[inline]
            pub fn offset(self, off: u64) -> Self {
                Self(self.0.wrapping_add(off))
            }
        }

        impl fmt::Display for $name {
            fn fmt(
                &self,
                f: &mut fmt::Formatter<'_>,
            ) -> fmt::Result {
                write!(f, "0x{:016x}", self.0)
            }
        }

        impl From<u64> for $name {
            #[inline]
            fn from(v: u64) -> Self {
                Self(v)
            }
        }

        impl From<$name> for u64 {
            #[inline]
            fn from(a: $name) -> Self {
                a.0
            }
        }
    };
}

addr_type! {
    /// Guest Physical Address.
    GPA
}

addr_type! {
    /// Guest Virtual Address.
    GVA
}

addr_type! {
    /// Host Virtual Address.
    HVA
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn construction_and_accessor() {
        let a = GPA::new(0x8000_0000);
        assert_eq!(a.0, 0x8000_0000);

        let b = GVA::new(42);
        assert_eq!(b.0, 42);

        let c = HVA::new(0);
        assert_eq!(c.0, 0);
    }

    #[test]
    fn offset_wrapping() {
        let a = GPA::new(10);
        assert_eq!(a.offset(20), GPA::new(30));

        // Wrapping at u64::MAX
        let max = GPA::new(u64::MAX);
        assert_eq!(max.offset(1), GPA::new(0));
        assert_eq!(max.offset(3), GPA::new(2));
    }

    #[test]
    fn display_formatting() {
        let a = GPA::new(0x1234);
        assert_eq!(format!("{a}"), "0x0000000000001234");

        let b = GVA::new(0);
        assert_eq!(format!("{b}"), "0x0000000000000000");

        let c = HVA::new(u64::MAX);
        assert_eq!(format!("{c}"), "0xffffffffffffffff");
    }

    #[test]
    fn ordering() {
        let a = GPA::new(1);
        let b = GPA::new(2);
        let c = GPA::new(1);
        assert!(a < b);
        assert!(b > a);
        assert_eq!(a, c);
    }

    #[test]
    fn from_into_conversions() {
        let a: GPA = 0x42u64.into();
        assert_eq!(a, GPA::new(0x42));
        let v: u64 = a.into();
        assert_eq!(v, 0x42);

        let b: GVA = GVA::from(100);
        let w: u64 = u64::from(b);
        assert_eq!(w, 100);

        let c: HVA = HVA::from(0xdead_beef);
        assert_eq!(u64::from(c), 0xdead_beef);
    }
}
