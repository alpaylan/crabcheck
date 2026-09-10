//! Procedural macros for [crabcheck](https://crates.io/crates/crabcheck).
//! Use them through the `crabcheck` crate (`crabcheck::Arbitrary` and
//! `#[crabcheck::test]`); this crate is an implementation detail.

use {
    proc_macro::TokenStream,
    proc_macro2::TokenStream as TokenStream2,
    quote::{
        format_ident,
        quote,
    },
    syn::{
        Data,
        DeriveInput,
        Expr,
        Fields,
        FnArg,
        Ident,
        ItemFn,
        Meta,
        Pat,
        Token,
        Visibility,
        parse_macro_input,
        punctuated::Punctuated,
        spanned::Spanned,
    },
};

/// Derive a default generator, shrinker and mutator for a struct or enum.
///
/// The type must be `Clone`. Every field is generated with its type's
/// `Arbitrary` default, or with `#[crabcheck(gen = <expression>)]`; the same
/// generator's `shrink` and `mutate` are used for that field, so candidates
/// stay in the field's range. Shrinking replaces one field at a time; enum
/// values also shrink to earlier variants that carry no data, so list the
/// simplest variant first. Mutation changes one field, or one time in eight
/// generates a fresh value (which can switch the variant).
///
/// `#[crabcheck(support = <path>)]` on the type names a predicate
/// `fn(&Self) -> bool`; candidates and mutants that fail it are dropped.
/// Use it for invariants that span fields.
///
/// ```ignore
/// #[derive(Debug, Clone, crabcheck::Arbitrary)]
/// #[crabcheck(support = Roll::is_valid)]
/// struct Roll {
///     #[crabcheck(gen = crabcheck::gens::int(1u8..=6))]
///     die: u8,
///     player: String,
/// }
/// ```
///
/// Recursive types (`enum Expr { Lit(i32), Add(Box<Expr>, Box<Expr>) }`)
/// derive fine but have no depth bound; for those, write the generator with
/// `crabcheck::gens::recursive` instead.
#[proc_macro_derive(Arbitrary, attributes(crabcheck))]
pub fn derive_arbitrary(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand_arbitrary(input).unwrap_or_else(|e| e.to_compile_error()).into()
}

/// The generator expression for one field: a `#[crabcheck(gen = ...)]`
/// override or the type's `Arbitrary`.
fn field_generator(field: &syn::Field) -> syn::Result<TokenStream2> {
    for attr in &field.attrs {
        if !attr.path().is_ident("crabcheck") {
            continue;
        }
        let meta: syn::MetaNameValue = attr.parse_args()?;
        if meta.path.is_ident("gen") {
            let expr = &meta.value;
            return Ok(quote!((#expr)));
        }
        return Err(syn::Error::new(
            meta.path.span(),
            "expected `#[crabcheck(gen = <expression>)]`",
        ));
    }
    let ty = &field.ty;
    Ok(quote!(<#ty as ::crabcheck::Arbitrary>::arbitrary()))
}

/// The `#[crabcheck(support = path)]` predicate on the type, if any.
fn support_predicate(input: &DeriveInput) -> syn::Result<Option<Expr>> {
    for attr in &input.attrs {
        if !attr.path().is_ident("crabcheck") {
            continue;
        }
        let meta: syn::MetaNameValue = attr.parse_args()?;
        if meta.path.is_ident("support") {
            return Ok(Some(meta.value));
        }
        return Err(syn::Error::new(
            meta.path.span(),
            "expected `#[crabcheck(support = <path to fn(&Self) -> bool>)]`",
        ));
    }
    Ok(None)
}

/// One shape of value: a struct or an enum variant, with how to build it,
/// take it apart, and reach its fields.
struct Shape {
    /// Path used to construct the value (`Type` or `Type::Variant`).
    path: TokenStream2,
    fields: Vec<ShapeField>,
    style: Style,
}

enum Style {
    Named,
    Unnamed,
    Unit,
}

struct ShapeField {
    /// Field name, or a synthetic name for tuple fields.
    binding: Ident,
    generator: TokenStream2,
}

fn shape(path: TokenStream2, fields: &Fields) -> syn::Result<Shape> {
    let (style, fields) = match fields {
        Fields::Named(named) => {
            (
                Style::Named,
                named
                    .named
                    .iter()
                    .map(|f| {
                        let ident = f.ident.clone().expect("named field");
                        Ok(ShapeField { binding: ident, generator: field_generator(f)? })
                    })
                    .collect::<syn::Result<Vec<_>>>()?,
            )
        },
        Fields::Unnamed(unnamed) => {
            (
                Style::Unnamed,
                unnamed
                    .unnamed
                    .iter()
                    .enumerate()
                    .map(|(i, f)| {
                        Ok(ShapeField {
                            binding: format_ident!("field_{i}"),
                            generator: field_generator(f)?,
                        })
                    })
                    .collect::<syn::Result<Vec<_>>>()?,
            )
        },
        Fields::Unit => (Style::Unit, Vec::new()),
    };
    Ok(Shape { path, fields, style })
}

impl Shape {
    /// `Path { a: <a>, b: <b> }` / `Path(<a>, <b>)` / `Path` from expressions.
    fn construct(&self, values: impl Iterator<Item = TokenStream2>) -> TokenStream2 {
        let path = &self.path;
        match self.style {
            Style::Named => {
                let inits = self.fields.iter().zip(values).map(|(f, v)| {
                    let name = &f.binding;
                    quote!(#name: #v)
                });
                quote!(#path { #(#inits),* })
            },
            Style::Unnamed => quote!(#path(#(#values),*)),
            Style::Unit => quote!(#path),
        }
    }

    /// A pattern binding every field to its `binding` name.
    fn pattern(&self) -> TokenStream2 {
        let path = &self.path;
        let names = self.fields.iter().map(|f| &f.binding);
        match self.style {
            Style::Named => quote!(#path { #(#names),* }),
            Style::Unnamed => quote!(#path(#(#names),*)),
            Style::Unit => quote!(#path),
        }
    }

    fn generate(&self) -> TokenStream2 {
        self.construct(self.fields.iter().map(|f| {
            let generator = &f.generator;
            quote!(g.draw(&#generator))
        }))
    }

    /// Candidates that replace one field by one of its shrinks, given the
    /// fields are bound by `pattern()`.
    fn shrink_arm(&self) -> TokenStream2 {
        let pattern = self.pattern();
        let per_field = self.fields.iter().enumerate().map(|(i, f)| {
            let generator = &f.generator;
            let binding = &f.binding;
            let rebuilt = self.construct(self.fields.iter().enumerate().map(|(j, other)| {
                let name = &other.binding;
                if i == j { quote!(candidate) } else { quote!(::core::clone::Clone::clone(#name)) }
            }));
            quote! {
                for candidate in #generator.shrink(#binding) {
                    out.push(#rebuilt);
                }
            }
        });
        quote! {
            #pattern => { #(#per_field)* }
        }
    }

    /// One mutated field, chosen at random, given the fields are bound.
    fn mutate_arm(&self) -> TokenStream2 {
        let pattern = self.pattern();
        let count = self.fields.len();
        if count == 0 {
            return quote!(#pattern => None,);
        }
        let per_field = self.fields.iter().enumerate().map(|(i, f)| {
            let generator = &f.generator;
            let binding = &f.binding;
            let rebuilt = self.construct(self.fields.iter().enumerate().map(|(j, other)| {
                let name = &other.binding;
                if i == j { quote!(mutated) } else { quote!(::core::clone::Clone::clone(#name)) }
            }));
            quote! {
                if which == #i && let Some(mutated) = #generator.mutate(#binding, g) {
                    return Some(#rebuilt);
                }
            }
        });
        let which = if count == 1 { quote!(0usize) } else { quote!((start + offset) % #count) };
        quote! {
            #pattern => {
                let start = g.index(#count);
                for offset in 0..#count {
                    let which: usize = #which;
                    let _ = (start, offset);
                    #(#per_field)*
                }
                None
            }
        }
    }
}

fn expand_arbitrary(input: DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let support = support_predicate(&input)?;
    let mut generics = input.generics.clone();
    for param in generics.type_params_mut() {
        param.bounds.push(syn::parse_quote!(::crabcheck::Arbitrary));
        param.bounds.push(syn::parse_quote!(::core::clone::Clone));
    }
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let shapes: Vec<Shape> = match &input.data {
        Data::Struct(data) => vec![shape(quote!(#name), &data.fields)?],
        Data::Enum(data) => {
            if data.variants.is_empty() {
                return Err(syn::Error::new_spanned(
                    name,
                    "cannot derive Arbitrary for an enum with no variants",
                ));
            }
            data.variants
                .iter()
                .map(|variant| {
                    let variant_name = &variant.ident;
                    shape(quote!(#name::#variant_name), &variant.fields)
                })
                .collect::<syn::Result<Vec<_>>>()?
        },
        Data::Union(_) => {
            return Err(syn::Error::new_spanned(name, "cannot derive Arbitrary for a union"));
        },
    };

    let generate = if shapes.len() == 1 {
        shapes[0].generate()
    } else {
        let count = shapes.len();
        let arms = shapes.iter().enumerate().map(|(index, s)| {
            let value = s.generate();
            quote!(#index => #value)
        });
        quote! {
            match g.int(0usize..#count) {
                #(#arms,)*
                _ => ::core::unreachable!(),
            }
        }
    };

    // Earlier data-less variants are candidates for every later variant.
    let unit_variants: Vec<TokenStream2> =
        shapes.iter().filter(|s| matches!(s.style, Style::Unit)).map(|s| s.path.clone()).collect();
    let shrink_arms = shapes.iter().map(Shape::shrink_arm);
    let mutate_arms = shapes.iter().map(Shape::mutate_arm);
    let variant_index_arms = shapes.iter().enumerate().map(|(i, s)| {
        let path = &s.path;
        match s.style {
            Style::Named => quote!(#path { .. } => #i),
            Style::Unnamed => quote!(#path(..) => #i),
            Style::Unit => quote!(#path => #i),
        }
    });
    let unit_candidates = if unit_variants.is_empty() {
        quote!()
    } else {
        let unit_index_arms =
            shapes.iter().enumerate().filter(|(_, s)| matches!(s.style, Style::Unit)).map(
                |(i, s)| {
                    let path = &s.path;
                    quote!(if #i < index { out.push(#path); })
                },
            );
        quote! {
            let index = match self { #(#variant_index_arms,)* };
            #(#unit_index_arms)*
        }
    };
    let apply_support_shrink = match &support {
        Some(pred) => quote!(out.retain(|candidate| (#pred)(candidate));),
        None => quote!(),
    };
    let apply_support_mutate = match &support {
        Some(pred) => quote!(.filter(|mutant| (#pred)(mutant))),
        None => quote!(),
    };
    let fresh_probability = if shapes.len() > 1 { quote!(0.125) } else { quote!(0.0) };

    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics ::crabcheck::Arbitrary for #name #ty_generics #where_clause {
            fn arbitrary() -> ::crabcheck::BoxGen<Self> {
                use ::crabcheck::GenExt as _;
                ::crabcheck::BoxGen::new((|g: &mut ::crabcheck::Source| -> Self { #generate }).typed())
            }
        }

        #[automatically_derived]
        impl #impl_generics ::crabcheck::Shrink for #name #ty_generics #where_clause {
            fn shrink(&self) -> ::std::vec::Vec<Self> {
                use ::crabcheck::Gen as _;
                let mut out: ::std::vec::Vec<Self> = ::std::vec::Vec::new();
                #unit_candidates
                match self { #(#shrink_arms)* }
                #apply_support_shrink
                out
            }
        }

        #[automatically_derived]
        impl #impl_generics ::crabcheck::Mutate for #name #ty_generics #where_clause {
            fn mutate(&self, g: &mut ::crabcheck::Source) -> ::core::option::Option<Self> {
                use ::crabcheck::Gen as _;
                if g.bool_with(#fresh_probability) {
                    return ::core::option::Option::Some(<Self as ::crabcheck::Arbitrary>::arbitrary().generate(g))#apply_support_mutate;
                }
                let mutated: ::core::option::Option<Self> = match self { #(#mutate_arms)* };
                mutated #apply_support_mutate
            }
        }
    })
}

/// Turn a function with typed parameters into a `#[test]` that checks it as
/// a property over `Arbitrary` inputs.
///
/// ```ignore
/// #[crabcheck::test]
/// fn sorting_is_idempotent(mut v: Vec<i32>) -> bool {
///     v.sort();
///     let once = v.clone();
///     v.sort();
///     v == once
/// }
///
/// #[crabcheck::test(tests = 1000, seed = 7)]
/// fn addition_commutes(a: u8, b: u8) -> bool {
///     u16::from(a) + u16::from(b) == u16::from(b) + u16::from(a)
/// }
/// ```
///
/// Accepted arguments: `tests = N`, `seed = N`, `max_size = N`, or
/// `config = <expression>` for a complete `Config`. The function may return
/// anything `Testable`. Other attributes (such as `#[ignore]`) are kept.
#[proc_macro_attribute]
pub fn test(args: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args with Punctuated::<Meta, Token![,]>::parse_terminated);
    let func = parse_macro_input!(item as ItemFn);
    expand_test(args, func).unwrap_or_else(|e| e.to_compile_error()).into()
}

fn expand_test(args: Punctuated<Meta, Token![,]>, func: ItemFn) -> syn::Result<TokenStream2> {
    let mut idents = Vec::new();
    let mut types = Vec::new();
    for arg in &func.sig.inputs {
        match arg {
            FnArg::Typed(typed) => {
                let ident = match &*typed.pat {
                    Pat::Ident(pat) => pat.ident.clone(),
                    other => {
                        return Err(syn::Error::new(
                            other.span(),
                            "crabcheck::test parameters must be plain identifiers",
                        ));
                    },
                };
                idents.push(ident);
                types.push((*typed.ty).clone());
            },
            FnArg::Receiver(receiver) => {
                return Err(syn::Error::new(
                    receiver.span(),
                    "crabcheck::test cannot be applied to methods",
                ));
            },
        }
    }
    if idents.is_empty() {
        return Err(syn::Error::new(
            func.sig.span(),
            "crabcheck::test needs at least one parameter to generate",
        ));
    }
    if idents.len() > 8 {
        return Err(syn::Error::new(
            func.sig.span(),
            "crabcheck::test supports up to 8 parameters; group them in a struct",
        ));
    }

    let mut config = quote!(::crabcheck::Config::default());
    for meta in args {
        let Meta::NameValue(pair) = meta else {
            return Err(syn::Error::new(
                meta.span(),
                "expected `key = value` (tests, seed, max_size, config)",
            ));
        };
        let value: &Expr = &pair.value;
        let key = pair.path.get_ident().map(ToString::to_string).unwrap_or_default();
        config = match key.as_str() {
            "tests" => quote!(#config.tests(#value)),
            "seed" => quote!(#config.seed(#value)),
            "max_size" => quote!(#config.max_size(#value)),
            "config" => quote!(#value),
            _ => {
                return Err(syn::Error::new(
                    pair.path.span(),
                    "unknown crabcheck::test argument; expected tests, seed, max_size or config",
                ));
            },
        };
    }

    let name = &func.sig.ident;
    let vis = &func.vis;
    let attrs = &func.attrs;
    let mut inner = func.clone();
    inner.attrs.clear();
    inner.vis = Visibility::Inherited;
    inner.sig.ident = format_ident!("__crabcheck_property");
    let call_args = idents.clone();

    Ok(quote! {
        #(#attrs)*
        #[test]
        #vis fn #name() {
            #inner
            let __config = {
                let mut __config: ::crabcheck::Config = #config;
                if __config.coverage.filter.is_empty() {
                    __config.coverage.filter = ::core::env!("CARGO_PKG_NAME").replace('-', "_");
                }
                __config
            };
            let __report = ::crabcheck::check(
                &__config,
                ::crabcheck::any::<(#(#types,)*)>(),
                |(#(#idents,)*)| __crabcheck_property(#(#call_args),*),
            );
            __report.assert_passed();
        }
    })
}
