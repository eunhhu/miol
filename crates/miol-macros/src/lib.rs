use proc_macro::TokenStream;
use quote::quote;

/// The `miol!` DSL entry point.
///
/// ```ignore
/// miol! {
///     // DSL content here
/// }
/// ```
#[proc_macro]
pub fn miol(input: TokenStream) -> TokenStream {
    let _input = syn::parse_macro_input!(input as proc_macro2::TokenStream);

    let expanded = quote! {
        {
            // TODO: implement DSL expansion
            ()
        }
    };

    expanded.into()
}
