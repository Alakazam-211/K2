//! Internal macros for unit enums.
//!
//! Every wire unit enum carries a `#[serde(other)] Unknown` fallback (serde rule 5), a
//! `pub const ALL` without it, `as_str`, `from_wire` and `Display`. The derive list is written
//! at each call site so the registry test can see it in the source.

/// Defines a wire unit enum whose JSON form is a string.
macro_rules! unit_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident = $wire:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        pub enum $name {
            $( $(#[$vmeta])* #[serde(rename = $wire)] $variant, )+
            /// A value this build does not know, sent by a newer peer. The server never sends it.
            #[serde(other, rename = "unknown")]
            #[schemars(skip)]
            Unknown,
        }

        impl $name {
            /// Every known value in wire order, without `Unknown`.
            pub const ALL: &'static [$name] = &[ $( $name::$variant ),+ ];

            /// The wire string. `Unknown` gives `"unknown"`.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $( $name::$variant => $wire, )+
                    $name::Unknown => "unknown",
                }
            }

            /// Parses a wire string. Strings this build does not know give `None`.
            pub fn from_wire(s: &str) -> Option<Self> {
                match s {
                    $( $wire => Some($name::$variant), )+
                    _ => None,
                }
            }

            /// Whether this is the forward-compat `Unknown` value.
            pub const fn is_unknown(self) -> bool {
                matches!(self, $name::Unknown)
            }
        }

        impl $crate::registry::UnitEnum for $name {
            fn all() -> &'static [Self] {
                Self::ALL
            }
            fn wire_str(self) -> &'static str {
                self.as_str()
            }
            fn is_unknown_value(self) -> bool {
                self.is_unknown()
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

/// Defines a wire unit enum that is also stored as a frozen `smallint` code (C8).
macro_rules! coded_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident = $wire:literal => $code:literal ),+ $(,)?
        }
    ) => {
        unit_enum! {
            $(#[$meta])*
            pub enum $name {
                $( $(#[$vmeta])* $variant = $wire ),+
            }
        }

        impl $name {
            /// The frozen storage code. `Unknown` gives `0`, which no value uses.
            pub const fn as_i16(self) -> i16 {
                match self {
                    $( $name::$variant => $code, )+
                    $name::Unknown => 0,
                }
            }

            /// Looks up a storage code. Codes this build does not know give `None`.
            pub const fn from_i16(code: i16) -> Option<Self> {
                match code {
                    $( $code => Some($name::$variant), )+
                    _ => None,
                }
            }
        }
    };
}
