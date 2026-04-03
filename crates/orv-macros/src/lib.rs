use proc_macro::TokenStream;
use quote::quote;

/// The `orv!` DSL entry point.
///
/// ```ignore
/// orv! {
///     // DSL content here
/// }
/// ```
#[proc_macro]
pub fn orv(input: TokenStream) -> TokenStream {
    let _input = syn::parse_macro_input!(input as proc_macro2::TokenStream);

    let expanded = quote! {
        {
            // TODO: implement DSL expansion
        }
    };

    expanded.into()
}
