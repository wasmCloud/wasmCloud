use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use proc_macro_error::{abort, proc_macro_error};
use quote::{format_ident, quote, ToTokens};
use syn::{
    parse::Result as ParseResult, parse_macro_input, spanned::Spanned, Attribute, Fields, Ident,
    Meta, NestedMeta,
};

/// extract traits from attribute
///  `#[services(Apple,Banana)]` returns vec![ Piano, Tuba ]
///  items in the vec are syn::Path, and may have more than one path segment,
///    as in instruments::Piano
///
fn attr_traits(attr: &Attribute, key: &str) -> Vec<syn::Path> {
    let mut traits = Vec::new();
    if attr.path.is_ident(key) {
        if let Ok(Meta::List(ref ml)) = attr.parse_meta() {
            for n in ml.nested.iter() {
                if let NestedMeta::Meta(Meta::Path(p)) = n {
                    traits.push(p.clone())
                }
            }
        }
    }
    traits
}

#[allow(dead_code)]
struct ReceiverDef {
    attrs: Vec<Attribute>,
    attrs_span: Span,
    ident: Ident,
    ident_span: Span,
    fields: Fields,
}

impl syn::parse::Parse for ReceiverDef {
    fn parse(input: syn::parse::ParseStream) -> ParseResult<Self> {
        let derive_input: syn::DeriveInput = input.parse()?;
        let attrs_span = derive_input.span();
        let syn::DeriveInput {
            attrs, ident, data, ..
        } = derive_input;
        let ident_span = ident.span();
        let fields = match data {
            syn::Data::Struct(data) => data.fields,
            _ => {
                return Err(syn::Error::new(
                    ident_span,
                    "derive macro only works for structs",
                ))
            }
        };
        Ok(ReceiverDef {
            attrs,
            attrs_span,
            ident,
            ident_span,
            fields,
        })
    }
}

#[proc_macro_error]
#[proc_macro_derive(Actor, attributes(services))]
pub fn derive_actor(input: TokenStream) -> TokenStream {
    let actor_receiver = parse_macro_input!(input as ReceiverDef);

    let mut traits = Vec::new();
    for attr in actor_receiver.attrs.iter() {
        traits.extend(attr_traits(attr, "services"));
    }
    if traits.is_empty() {
        abort!(
            actor_receiver.attrs_span,
            "Missing list of traits. try `#[services(Trait1,Trait2)]`"
        );
    }
    let actor_ident = actor_receiver.ident;
    let dispatch_impl = gen_dispatch(&traits, &actor_ident);
    let output = quote!(

    #[link(wasm_import_module = "wapc")]
    extern "C" {
        pub fn __guest_response(ptr: *const u8, len: usize);
        pub fn __guest_error(ptr: *const u8, len: usize);
        pub fn __guest_request(op_ptr: *const u8, ptr: *const u8);
    }

    #[no_mangle]
    pub extern "C" fn __actor_api_version() -> u32 {
        wasmcloud_weld_rpc::WELD_RPC_VERSION
    }

    #[no_mangle]
    pub extern "C" fn __guest_call(op_len: i32, req_len: i32) -> i32 {
        use std::slice;

        let buf: Vec<u8> = Vec::with_capacity(req_len as _);
        let req_ptr = buf.as_ptr();

        let opbuf: Vec<u8> = Vec::with_capacity(op_len as _);
        let op_ptr = opbuf.as_ptr();

        let (slice, op) = unsafe {
            __guest_request(op_ptr, req_ptr);
            (
                slice::from_raw_parts(req_ptr, req_len as _),
                slice::from_raw_parts(op_ptr, op_len as _),
            )
        };
        let method = String::from_utf8_lossy(op);
        let context = context::Context::default();
        let actor = #actor_ident ::default();
        let resp = futures::executor::block_on({
            MessageDispatch::dispatch(
                &actor,
                &context,
                Message {
                    method: &method,
                    arg: std::borrow::Cow::Borrowed(slice),
                },
            )
        });
        match resp {
            Ok(Message { arg, .. }) => {
                unsafe {
                    __guest_response(arg.as_ptr(), arg.len() as _);
                }
                1
            }
            Err(e) => {
                let errmsg = format!("Guest call failed for method {}: {}",
                        &method, e);
                unsafe {
                    __guest_error(errmsg.as_ptr(), errmsg.len() as _);
                }
                0
            }
        }
    }

       #dispatch_impl
    ); // end quote

    // struct #actor_ident { #fields }
    output.into()
}
/*



*/

fn gen_dispatch(traits: &[syn::Path], ident: &Ident) -> TokenStream2 {
    let mut methods = Vec::new();
    let mut methods_legacy = Vec::new();
    let mut trait_receiver_impl = Vec::new();
    //let ident_name = ident.to_string();

    for path in traits.iter() {
        let path_str = path.segments.to_token_stream().to_string();
        let id = format_ident!("{}Receiver", &path_str);
        //let quoted_path = format!("\"{}\"", &path_str);
        methods.push(quote!(
            #path_str => #id::dispatch(self, ctx, &message).await
        ));
        methods_legacy.push(quote!(
            match #id::dispatch(self, ctx, &message).await {
                Err(RpcError::MethodNotHandled(_)) => {}, // continue
                res => return res, // either Ok(_) or Err(_)
            };
        ));
        trait_receiver_impl.push(quote!(
            impl #id for #ident { }
        ));
    }

    quote!(
        #[async_trait]
        impl MessageDispatch for #ident {
            async fn dispatch(
                &self,
                ctx: &context::Context<'_>,
                message: Message<'_>,
            ) -> Result<Message<'static>, RpcError> {
                let (trait_name, trait_method) = message
                    .method
                    .rsplit_once('.')
                    .unwrap_or(("_", message.method));

                let message = Message {
                    method: trait_method,
                    arg: message.arg,
                };
                match trait_name {
                   #( #methods, )*

                    "_" => {
                        // legacy handlers  - compatibility with no Trait prefix
                        #( #methods_legacy )*
                        Err(RpcError::MethodNotHandled(message.method.to_string()))
                    },
                    _ => Err(RpcError::MethodNotHandled(
                            format!("{} - unknown trait", message.method)))
                }
            }
        }

      #( #trait_receiver_impl )*
    )
}

#[proc_macro_error]
#[proc_macro_derive(Provider, attributes(services))]
pub fn derive_provider(input: TokenStream) -> TokenStream {
    let provider_receiver = parse_macro_input!(input as ReceiverDef);

    let mut traits = Vec::new();
    for attr in provider_receiver.attrs.iter() {
        traits.extend(attr_traits(attr, "services"));
    }
    if traits.is_empty() {
        abort!(
            provider_receiver.attrs_span,
            "Missing list of traits. try `#[services(Trait1,Trait2)]`"
        );
    }
    let ident = provider_receiver.ident;
    //let fields = actor_receiver.fields;
    let dispatch_impl = gen_dispatch(&traits, &ident);
    let output = quote!(

    impl wasmcloud_provider_core::CapabilityProvider for #ident {
        /// This function will be called on the provider when the host runtime is ready and has
        /// configured a dispatcher. This function is only ever
        /// called _once_ for a capability provider, regardless of the number of actors being
        /// managed in the host
        fn configure_dispatch( &self, dispatcher: Box<dyn provider::Dispatcher>,
             ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
          let mut lock = self.dispatcher.write().unwrap();
          *lock = Some(dispatcher);
          Ok(())
        }

        /// Invoked when an actor has requested that a provider perform a given operation
        fn handle_call(
            &self,
            actor: &str,
            op: &str,
            arg: &[u8],
        ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
            let ctx = &context::Context {
                actor: Some(actor),
                ..Default::default()
            };
            let response = futures::executor::block_on(MessageDispatch::dispatch(
                self,
                &ctx,
                Message {
                    method: op,
                    arg: std::borrow::Cow::Borrowed(arg),
                },
            ))?;
            Ok(response.arg.to_vec())
        }

        /// This function is called to let the capability provider know that it is being removed
        /// from the host runtime. This gives the provider an opportunity to clean up any
        /// resources and stop any running threads.
        /// WARNING: do not do anything in this function that can
        /// cause a panic, including attempting to write to STDOUT while the host process is terminating
        fn stop(&self) {
                #ident :: stop(&self);
        }
    }

    #dispatch_impl

        );
    output.into()
}
