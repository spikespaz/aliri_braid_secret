use std::borrow::Cow;

use self::check_mode::{CheckMode, IndefiniteCheckMode};
use self::impls::{Impls, DelegatingImplOption, ImplOption};
use quote::{ToTokens, TokenStreamExt, format_ident};
use symbol::{parse_lit_into_string, parse_lit_into_type};
use syn::spanned::Spanned;

pub use self::owned::OwnedCodeGen;
pub use self::borrowed::RefCodeGen;

mod borrowed;
mod check_mode;
mod impls;
mod owned;
mod symbol;

pub type AttrList<'a> = syn::punctuated::Punctuated<&'a syn::NestedMeta, syn::Token![,]>;

pub struct Params<'a> {
    ref_ty: Option<syn::Type>,
    ref_doc: Vec<Cow<'a, syn::Lit>>,
    ref_attrs: AttrList<'a>,
    owned_attrs: AttrList<'a>,
    check_mode: IndefiniteCheckMode,
    impls: Impls,
}

impl<'a> Default for Params<'a> {
    fn default() -> Self {
        Self {
            ref_ty: None,
            ref_doc: Vec::new(),
            ref_attrs: AttrList::new(),
            owned_attrs: AttrList::new(),
            check_mode: IndefiniteCheckMode::None,
            impls: Impls::default(),
        }
    }
}

impl<'a> Params<'a> {
    pub fn parse(args: &'a syn::AttributeArgs) -> Result<Self, syn::Error> {
        let mut params = Self::default();

        for arg in args {
            match arg {
                syn::NestedMeta::Meta(syn::Meta::NameValue(nv)) if nv.path == symbol::REF => {
                    params.ref_ty = Some(parse_lit_into_type(symbol::REF, &nv.lit)?);
                }
                syn::NestedMeta::Meta(syn::Meta::NameValue(nv)) if nv.path == symbol::VALIDATOR => {
                    let validator = parse_lit_into_type(symbol::VALIDATOR, &nv.lit)?;
                    params.check_mode
                        .try_set_validator(Some(validator))
                        .map_err(|s| syn::Error::new_spanned(arg, s))?;
                }
                syn::NestedMeta::Meta(syn::Meta::NameValue(nv)) if nv.path == symbol::NORMALIZER => {
                    let normalizer = parse_lit_into_type(symbol::NORMALIZER, &nv.lit)?;
                    params.check_mode
                        .try_set_normalizer(Some(normalizer))
                        .map_err(|s| syn::Error::new_spanned(arg, s))?;
                }
                syn::NestedMeta::Meta(syn::Meta::NameValue(nv)) if nv.path == symbol::REF_DOC => {
                    params.ref_doc.push(Cow::Borrowed(&nv.lit));
                }
                syn::NestedMeta::Meta(syn::Meta::List(nv)) if nv.path == symbol::REF_ATTR => {
                    params.ref_attrs.extend(nv.nested.iter());
                }
                syn::NestedMeta::Meta(syn::Meta::List(nv)) if nv.path == symbol::OWNED_ATTR => {
                    params.owned_attrs.extend(nv.nested.iter());
                }
                syn::NestedMeta::Meta(syn::Meta::NameValue(nv)) if nv.path == symbol::DEBUG => {
                    params.impls.debug = parse_lit_into_string(symbol::DEBUG, &nv.lit)?
                        .parse::<DelegatingImplOption>()
                        .map_err(|e| syn::Error::new_spanned(&arg, e.to_owned()))?
                        .into();
                }
                syn::NestedMeta::Meta(syn::Meta::NameValue(nv)) if nv.path == symbol::DISPLAY => {
                    params.impls.display = parse_lit_into_string(symbol::DISPLAY, &nv.lit)?
                        .parse::<DelegatingImplOption>()
                        .map_err(|e| syn::Error::new_spanned(&arg, e.to_owned()))?
                        .into();
                }
                syn::NestedMeta::Meta(syn::Meta::NameValue(nv)) if nv.path == symbol::CLONE => {
                    params.impls.clone = parse_lit_into_string(symbol::CLONE, &nv.lit)?
                        .parse::<ImplOption>()
                        .map_err(|e| syn::Error::new_spanned(&arg, e.to_owned()))?
                        .into();
                }
                syn::NestedMeta::Meta(syn::Meta::NameValue(nv)) if nv.path == symbol::SERDE => {
                    params.impls.serde = parse_lit_into_string(symbol::SERDE, &nv.lit)?
                        .parse::<ImplOption>()
                        .map_err(|e| syn::Error::new_spanned(&arg, e.to_owned()))?
                        .into();
                }
                syn::NestedMeta::Meta(syn::Meta::Path(p)) if p == symbol::SERDE => {
                    params.impls.serde = ImplOption::Implement.into();
                }
                syn::NestedMeta::Meta(syn::Meta::Path(p)) if p == symbol::VALIDATOR => {
                    params.check_mode
                        .try_set_validator(None)
                        .map_err(|s| syn::Error::new_spanned(arg, s))?;
                }
                syn::NestedMeta::Meta(syn::Meta::Path(p)) if p == symbol::NORMALIZER => {
                    params.check_mode
                        .try_set_normalizer(None)
                        .map_err(|s| syn::Error::new_spanned(arg, s))?;
                }
                syn::NestedMeta::Meta(syn::Meta::Path(ref path) | syn::Meta::NameValue(syn::MetaNameValue {
                    ref path,
                    ..
                })) => {
                    return Err(syn::Error::new_spanned(
                        &arg,
                        format!("unsupported argument `{}`", path.to_token_stream()),
                    ));
                }
                _ => {
                    return Err(syn::Error::new_spanned(
                        &arg,
                        "unsupported argument".to_string(),
                    ));
                }
            }
        }

        Ok(params)
    }

    pub fn build(self, body: &'a mut syn::ItemStruct) -> Result<CodeGen, syn::Error> {
        let Params {
            ref_ty,
            mut ref_doc,
            ref_attrs,
            owned_attrs,
            check_mode,
            impls,
        } = self;

        create_field_if_none(&mut body.fields);
        let (wrapped_type, field_ident, field_attrs) = get_field_info(&body.fields)?;
        let owned_ty = &body.ident;
        if ref_doc.is_empty() {
            ref_doc.push(Cow::Owned(syn::LitStr::new(&format!("A reference to a borrowed [`{}`]", owned_ty), proc_macro2::Span::call_site()).into()));
        }
        let ref_ty = ref_ty.unwrap_or_else(|| infer_ref_type_from_owned_name(owned_ty));
        let check_mode = check_mode.infer_validator_if_missing(owned_ty);
        let field = Field {
            attrs: field_attrs,
            name: field_ident.map_or(FieldName::Unnamed, FieldName::Named),
            ty: wrapped_type,
        };

        Ok(CodeGen {
            check_mode,
            body,
            field,

            owned_attrs,

            ref_doc,
            ref_attrs,
            ref_ty,

            impls,
        })
    }
}

pub struct CodeGen<'a> {
    check_mode: CheckMode,
    body: &'a syn::ItemStruct,
    field: Field<'a>,

    owned_attrs: AttrList<'a>,

    ref_doc: Vec<Cow<'a, syn::Lit>>,
    ref_attrs: AttrList<'a>,
    ref_ty: syn::Type,

    impls: Impls,
}

impl<'a> CodeGen<'a> {
    pub fn generate(&self) -> proc_macro2::TokenStream {
        let owned = self.owned().tokens();
        let ref_ = self.borrowed().tokens();

        quote::quote! {
            #owned
            #ref_
        }
    }


    pub fn owned(&self) -> OwnedCodeGen {
        OwnedCodeGen {
            common_attrs: &self.body.attrs,
            check_mode: &self.check_mode,
            body: self.body,
            field: self.field,
            attrs: &self.owned_attrs,
            ty: &self.body.ident,
            ref_ty: &self.ref_ty,
            impls: &self.impls,
        }
    }

    pub fn borrowed(&self) -> RefCodeGen {
        RefCodeGen {
            doc: &self.ref_doc,
            common_attrs: &self.body.attrs,
            check_mode: &self.check_mode,
            vis: &self.body.vis,
            field: self.field,
            attrs: &self.ref_attrs,
            ty: &self.ref_ty,
            ident: syn::Ident::new(&self.ref_ty.to_token_stream().to_string(), self.ref_ty.span()),
            owned_ty: &self.body.ident,
            impls: &self.impls,
        }
    }
}

fn infer_ref_type_from_owned_name(name: &syn::Ident) -> syn::Type {
    let name_str = name.to_string();
    if name_str.ends_with("Buf") {
        syn::Type::Path(syn::TypePath {
            qself: None,
            path: syn::Path::from(format_ident!("{}", name_str[..name_str.len() - 3])),
        })
    } else {
        syn::Type::Path(syn::TypePath {
            qself: None,
            path: syn::Path::from(format_ident!("{}Ref", name_str)),
        })
    }
}

fn create_field_if_none(
    fields: &mut syn::Fields,
) {
    if fields.is_empty() {
        let field = syn::Field {
            vis: syn::Visibility::Inherited,
            attrs: Vec::new(),
            colon_token: None,
            ident: None,
            ty: syn::Type::Verbatim(syn::Ident::new("String", proc_macro2::Span::call_site()).into_token_stream()),
        };

        *fields = syn::Fields::Unnamed(syn::FieldsUnnamed {
            paren_token: syn::token::Paren::default(),
            unnamed: std::iter::once(field).collect(),
        });
    }
}

fn get_field_info(fields: &syn::Fields) -> Result<(&syn::Type, Option<&syn::Ident>, &[syn::Attribute]), syn::Error> {
    let mut iter = fields.iter();
    let field = iter.next().unwrap();

    if iter.next().is_some() {
        return Err(syn::Error::new_spanned(
            &fields,
            "typed string can only have one field",
        ))
    }

    Ok((&field.ty, field.ident.as_ref(), &field.attrs))
}

#[derive(Copy, Clone)]
pub struct Field<'a> {
    pub attrs: &'a [syn::Attribute],
    pub name: FieldName<'a>,
    pub ty: &'a syn::Type,
}

impl<'a> Field<'a> {
    fn self_constructor(self) -> SelfConstructorImpl<'a> {
        SelfConstructorImpl(self)
    }
}

#[derive(Copy, Clone)]
pub enum FieldName<'a> {
    Named(&'a syn::Ident),
    Unnamed,
}

impl<'a> FieldName<'a> {
    fn constructor_delimiter(self) -> proc_macro2::Delimiter {
        match self {
            FieldName::Named(_) => proc_macro2::Delimiter::Brace,
            FieldName::Unnamed => proc_macro2::Delimiter::Parenthesis,
        }
    }

    fn input_name(self) -> proc_macro2::Ident {
        match self {
            FieldName::Named(name) => name.clone(),
            FieldName::Unnamed => proc_macro2::Ident::new("raw", proc_macro2::Span::call_site()),
        }
    }
}

impl<'a> ToTokens for FieldName<'a> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        match self {
            Self::Named(ident) => ident.to_tokens(tokens),
            Self::Unnamed => tokens.append(proc_macro2::Literal::u8_unsuffixed(0)),
        }
    }
}

struct SelfConstructorImpl<'a>(Field<'a>);

impl<'a> ToTokens for SelfConstructorImpl<'a> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let Self(field) = self;
        tokens.append(proc_macro2::Ident::new("Self", proc_macro2::Span::call_site()));
        tokens.append(proc_macro2::Group::new(field.name.constructor_delimiter(), field.name.input_name().into_token_stream()));
    }
}
