/// Generate a roundtrip property to assert that Cbor encoder and decoder for a given type can
/// safely be called in sequence and yield the original input.
///
/// Requires:
/// - proptest
///
/// Usage:
///
/// # ```
/// # prop_cbor_roundtrip!(MyType, my_strategy())
/// #
/// # // Or with an explicit test title in case a module contains multiple calls to the macro:
/// # prop_cbor_roundtrip!(prop_cbor_roundtrip_MyType, MyType, my_strategy())
/// # ```
#[macro_export]
macro_rules! prop_cbor_roundtrip {
    ($title:ident, $ty:ty, $strategy:expr) => {
        proptest::proptest! {
            #[test]
            fn $title(val in $strategy) {
                let bytes = amaru_kernel::to_cbor(&val);
                proptest::prop_assert_eq!(Some(val), amaru_kernel::from_cbor::<$ty>(&bytes));
            }
        }
    };

    ($ty:ty, $strategy:expr) => {
        prop_cbor_roundtrip!(prop_cbor_roundtrip, $ty, $strategy);
    };
}
