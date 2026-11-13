//! Enums whose values have names in the documents of the tool.

/// Defines an enum whose values have names in documents, with `ALL`, every
/// value in order, `name()`, the name of a value, and `Display` as the name.
macro_rules! named_enum {
    (
        $(#[$meta:meta])*
        pub enum $ty:ident { $($(#[$doc:meta])* $value:ident = $name:literal,)* }
    ) => {
        $(#[$meta])*
        pub enum $ty { $($(#[$doc])* $value,)* }

        impl $ty {
            /// Every value, in order.
            pub const ALL: [$ty; [$($name),*].len()] = [$($ty::$value),*];

            /// The name of the value in documents.
            pub fn name(self) -> &'static str {
                match self { $($ty::$value => $name,)* }
            }
        }

        impl std::fmt::Display for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.name())
            }
        }
    };
}
pub(crate) use named_enum;
