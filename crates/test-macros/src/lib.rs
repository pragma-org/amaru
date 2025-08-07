use proc_macro::TokenStream;
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    parse_macro_input, Ident, ItemFn, LitStr, Result, Token,
};

/// Parsed arguments for `#[must_panic(expected = "...")]`
struct MustPanicArgs {
    pub expected: Option<String>,
}

impl Parse for MustPanicArgs {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut expected: Option<String> = None;
        while !input.is_empty() {
            // Parse identifier (should be "expected")
            let ident: Ident = input.parse()?;
            // Expect '='
            input.parse::<Token![=]>()?;
            // Parse string literal
            let lit: LitStr = input.parse()?;
            if ident == "expected" {
                expected = Some(lit.value());
            } else {
                return Err(syn::Error::new(
                    ident.span(),
                    format!("unknown argument `{}`", ident),
                ));
            }
            // Consume optional trailing comma
            let _ = input.parse::<Token![,]>();
        }
        Ok(MustPanicArgs { expected })
    }
}

#[proc_macro_attribute]
pub fn must_panic(attr: TokenStream, item: TokenStream) -> TokenStream {
    // Parse the attribute arguments into `MustPanicArgs`
    let args = parse_macro_input!(attr as MustPanicArgs);
    let expected = args.expected;

    // Parse the input function
    let input_fn = parse_macro_input!(item as ItemFn);
    let fn_name = &input_fn.sig.ident;
    let fn_attrs = &input_fn.attrs;
    let fn_vis = &input_fn.vis;
    let fn_sig = &input_fn.sig;
    let fn_block = input_fn.block;

    // Generate the test body based on whether an expected message was provided
    let body = if let Some(expected_msg) = expected {
        quote! {
            use std::panic;

            let original_hook = panic::take_hook();
            panic::set_hook(Box::new(|_| {})); // disable panic output

            let result = panic::catch_unwind(panic::AssertUnwindSafe(|| #fn_block ));

            panic::set_hook(original_hook);

            match result {
                Ok(_) => panic!("test `{}` did not panic as expected", stringify!(#fn_name)),
                Err(e) => {
                    let msg = if let Some(s) = e.downcast_ref::<&str>() {
                        *s
                    } else if let Some(s) = e.downcast_ref::<String>() {
                        s.as_str()
                    } else {
                        "<non-string panic>"
                    };
                    if !msg.contains(#expected_msg) {
                        panic!(
                            "test `{}` panicked with unexpected message:\n  got: {:?}\n  expected to contain: {:?}",
                            stringify!(#fn_name),
                            msg,
                            #expected_msg
                        );
                    }
                }
            }
        }
    } else {
        quote! {
            use std::panic;

            let original_hook = panic::take_hook();
            panic::set_hook(Box::new(|_| {})); // disable panic output

            let result = panic::catch_unwind(panic::AssertUnwindSafe(|| #fn_block ));

            panic::set_hook(original_hook);

            if result.is_ok() {
                panic!("test `{}` did not panic as expected", stringify!(#fn_name));
            }
        }
    };

    // Assemble the final expanded function
    let expanded = quote! {
        #[allow(non_snake_case)]
        #(#fn_attrs)*
        #fn_vis #fn_sig {
            #body
        }
    };

    expanded.into()
}
