//! `code_enum!`: fieldless enums with a stable string code.
//!
//! The code is what the catalog stores in its `TEXT` columns and what serde reads and writes, so the
//! two can never drift apart. Codes are part of the frozen contract: never change an existing one.

/// Defines a fieldless enum whose variants map 1:1 to stable string codes.
///
/// Generates `code()`, `from_code()`, `ALL`, `Display`, `FromStr` (error: `CoreError::UnknownCode`)
/// and serde impls that use the code.
macro_rules! code_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident => $code:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        $vis enum $name {
            $( $(#[$vmeta])* $variant ),+
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            /// The stable string code (catalog column value and serde representation).
            pub const fn code(self) -> &'static str {
                match self {
                    $( $name::$variant => $code ),+
                }
            }

            /// Decodes a code produced by [`Self::code`]. Exact, case-sensitive match.
            pub fn from_code(code: &str) -> Option<Self> {
                match code {
                    $( $code => Some($name::$variant), )+
                    _ => None,
                }
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.code())
            }
        }

        impl ::std::str::FromStr for $name {
            type Err = $crate::CoreError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::from_code(s).ok_or_else(|| $crate::CoreError::UnknownCode {
                    kind: stringify!($name),
                    code: s.to_owned(),
                })
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.code())
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let code = <::std::string::String as ::serde::Deserialize>::deserialize(deserializer)?;
                code.parse().map_err(::serde::de::Error::custom)
            }
        }
    };
}

pub(crate) use code_enum;
