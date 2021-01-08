use crate::dispatch::{Invocation, InvocationResponse};
use crate::middleware::Middleware;
use crate::Result;
use actix::Recipient;

/// Executes a chain of pre-invoke handlers for a capability
pub(crate) fn run_capability_pre_invoke(
    inv: &Invocation,
    middlewares: &[Box<dyn Middleware>],
) -> Result<()> {
    for m in middlewares {
        if let Err(e) = m.capability_pre_invoke(&inv) {
            error!("Capability middleware pre-invoke failure: {}", e);
            return Err(e);
        }
    }
    Ok(())
}

/// Executes a chain of post-invoke handlers for a capability
pub(crate) fn run_capability_post_invoke(
    resp: InvocationResponse,
    middlewares: &[Box<dyn Middleware>],
) -> Result<InvocationResponse> {
    let mut cur_resp = resp;
    for m in middlewares {
        match m.capability_post_invoke(cur_resp) {
            Ok(ir) => cur_resp = ir.clone(),
            Err(e) => {
                error!("Capability middleware post-invoke failure: {}", e);
                return Err(e);
            }
        }
    }
    Ok(cur_resp)
}

/// Executes a chain of pre-invoke handlers for an actor
pub(crate) fn run_actor_pre_invoke(
    inv: &Invocation,
    middlewares: &[Box<dyn Middleware>],
) -> Result<()> {
    for m in middlewares {
        if let Err(e) = m.actor_pre_invoke(&inv) {
            error!("Actor pre-invoke middleware failure: {}", e);
            return Err(e);
        }
    }
    Ok(())
}

/// Executes a chain of post-invoke handlers for an actor
pub(crate) fn run_actor_post_invoke(
    resp: InvocationResponse,
    middlewares: &[Box<dyn Middleware>],
) -> Result<InvocationResponse> {
    let mut cur_resp = resp;
    for m in middlewares {
        match m.actor_post_invoke(cur_resp) {
            Ok(i) => cur_resp = i.clone(),
            Err(e) => {
                error!("Actor post-invoke middleware failure: {}", e);
                return Err(e);
            }
        }
    }
    Ok(cur_resp)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::Middleware;
    use crate::dispatch::{Invocation, InvocationResponse, WasmCloudEntity};
    use crate::Result;
    use actix::prelude::*;
    use wascap::prelude::KeyPair;

    struct HappyActor {
        inv_count: u32,
    }

    impl Actor for HappyActor {
        type Context = SyncContext<Self>;
    }

    impl Handler<Invocation> for HappyActor {
        type Result = InvocationResponse;

        fn handle(&mut self, msg: Invocation, _ctx: &mut Self::Context) -> Self::Result {
            self.inv_count = self.inv_count + 1;
            InvocationResponse::success(&msg, vec![])
        }
    }

    #[derive(Clone)]
    struct IncMiddleware {
        pre: &'static AtomicUsize,
        post: &'static AtomicUsize,
        cap_pre: &'static AtomicUsize,
        cap_post: &'static AtomicUsize,
    }

    impl Middleware for IncMiddleware {
        fn actor_pre_invoke(&self, _inv: &Invocation) -> Result<()> {
            self.pre.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn actor_post_invoke(&self, response: InvocationResponse) -> Result<InvocationResponse> {
            self.post.fetch_add(1, Ordering::SeqCst);
            Ok(response)
        }
        fn capability_pre_invoke(&self, _inv: &Invocation) -> Result<()> {
            self.cap_pre.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn capability_post_invoke(
            &self,
            response: InvocationResponse,
        ) -> Result<InvocationResponse> {
            self.cap_post.fetch_add(1, Ordering::SeqCst);
            Ok(response)
        }
    }

    static PRE: AtomicUsize = AtomicUsize::new(0);
    static POST: AtomicUsize = AtomicUsize::new(0);
    static CAP_PRE: AtomicUsize = AtomicUsize::new(0);
    static CAP_POST: AtomicUsize = AtomicUsize::new(0);

    static FULL: AtomicUsize = AtomicUsize::new(0);

    #[actix_rt::test]
    async fn simple_add() {
        let inc_mid = IncMiddleware {
            pre: &PRE,
            post: &POST,
            cap_pre: &CAP_PRE,
            cap_post: &CAP_POST,
        };
        let hk = KeyPair::new_server();

        let mids: Vec<Box<dyn Middleware>> = vec![Box::new(inc_mid)];
        let inv = Invocation::new(
            &hk,
            WasmCloudEntity::Actor("test".to_string()),
            WasmCloudEntity::Capability {
                id: "Vxxx".to_string(),
                contract_id: "testing:sample".to_string(),
                link_name: "default".to_string(),
            },
            "testing",
            b"abc1234".to_vec(),
        );
        let res = super::run_actor_pre_invoke(&inv.clone(), &mids);
        assert!(res.is_ok());
        let res2 = super::run_actor_pre_invoke(&inv, &mids);
        assert!(res2.is_ok());
        assert_eq!(PRE.fetch_add(0, Ordering::SeqCst), 2);
    }
}
