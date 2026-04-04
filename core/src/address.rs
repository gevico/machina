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
