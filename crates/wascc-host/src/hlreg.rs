use actix::{Addr, System, SystemService};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::any::{Any, TypeId};
use std::collections::HashMap;

type AnyMap = HashMap<TypeId, Box<dyn Any + Send>>;
type ServiceMap = HashMap<String, AnyMap>;

static SREG: Lazy<Mutex<ServiceMap>> = Lazy::new(|| Mutex::new(HashMap::new()));

pub(crate) trait HostLocalSystemService: SystemService {
    fn from_hostlocal_registry(hostid: &str) -> Addr<Self> {
        System::with_current(|sys| {
            let mut sreg = SREG.lock();
            let reg = sreg
                .entry(hostid.to_string())
                .or_insert_with(|| HashMap::new());

            if let Some(addr) = reg.get(&TypeId::of::<Self>()) {
                if let Some(addr) = addr.downcast_ref::<Addr<Self>>() {
                    return addr.clone();
                }
            }

            let addr = Self::start_service(sys.arbiter());
            reg.insert(TypeId::of::<Self>(), Box::new(addr.clone()));
            addr
        })
    }
}
