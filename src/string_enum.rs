//! A closed set of strings as an enum, each variant's text written once.
//!
//! The text is what the JSON output, the log and the human report show, so
//! `as_str`, `Display`, `Serialize` and `Deserialize` all read it from the
//! one table.

macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $($(#[$variant_meta:meta])* $variant:ident => $text:literal,)+
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        $vis enum $name {
            $($(#[$variant_meta])* $variant,)+
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                }
            }

            /// The variant whose text is `text`.
            pub fn parse(text: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|v| v.as_str() == text)
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = <::std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
                Self::parse(&text).ok_or_else(|| {
                    let expected: Vec<&str> = Self::ALL.iter().map(|v| v.as_str()).collect();
                    ::serde::de::Error::custom(format!(
                        "unknown {} {text:?}, expected one of {}",
                        stringify!($name),
                        expected.join(", ")
                    ))
                })
            }
        }
    };
}
