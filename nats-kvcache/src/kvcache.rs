use std::collections::HashMap;
use std::collections::HashSet;
use std::error::Error;
use std::result::Result;

#[derive(Debug, Clone, PartialEq)]
pub enum KeyValueItem {
    Atomic(i32),
    Scalar(String),
    List(Vec<String>),
    Set(HashSet<String>),
}

#[derive(Clone, Debug)]
pub(crate) struct KeyValueStore {
    items: HashMap<String, KeyValueItem>,
}

impl KeyValueStore {
    pub fn new() -> Self {
        KeyValueStore {
            items: HashMap::new(),
        }
    }

    pub fn incr(&mut self, key: &str, value: i32) -> Result<i32, Box<dyn Error>> {
        let mut orig = 0;
        self.items
            .entry(key.to_string())
            .and_modify(|v| {
                if let KeyValueItem::Atomic(ref x) = v {
                    orig = *x;
                    *v = KeyValueItem::Atomic(x + value);
                }
            })
            .or_insert(KeyValueItem::Atomic(value));
        Ok(orig + value)
    }

    pub fn del(&mut self, key: &str) -> Result<(), Box<dyn Error>> {
        self.items.remove(key);
        Ok(())
    }

    pub fn exists(&self, key: &str) -> Result<bool, Box<dyn Error>> {
        Ok(self.items.contains_key(key))
    }

    pub fn get(&self, key: &str) -> Result<String, Box<dyn Error>> {
        self.items.get(key).map_or_else(
            || Err("No such key".into()),
            |v| {
                if let KeyValueItem::Scalar(ref s) = v {
                    Ok(s.clone())
                } else if let KeyValueItem::Atomic(x) = v {
                    Ok(x.to_string())
                } else {
                    Err("Attempt to fetch non-scalar as a scalar".into())
                }
            },
        )
    }

    pub fn lrange(&self, key: &str, start: i32, stop: i32) -> Result<Vec<String>, Box<dyn Error>> {
        let start = start.max(0);
        self.items.get(key).map_or_else(
            || Ok(vec![]),
            |v| {
                if let KeyValueItem::List(l) = v {
                    let stop = stop.min(l.len() as _);
                    Ok(l.as_slice()[start as _..stop as _].to_vec())
                } else {
                    Err("Attempt to fetch non-list".into())
                }
            },
        )
    }

    pub fn lpush(&mut self, key: &str, value: String) -> Result<i32, Box<dyn Error>> {
        let mut len = 1;
        self.items
            .entry(key.to_string())
            .and_modify(|v| {
                if let KeyValueItem::List(ref l) = v {
                    let mut list = Vec::new();
                    list.extend_from_slice(&l);
                    list.push(value.clone());
                    len = list.len();
                    *v = KeyValueItem::List(list);
                }
            })
            .or_insert_with(|| KeyValueItem::List(vec![value]));
        Ok(len as _)
    }

    pub fn set(&mut self, key: &str, value: String) -> Result<(), Box<dyn Error>> {
        self.items
            .entry(key.to_string())
            .and_modify(|v| {
                if let KeyValueItem::Scalar(_) = v {
                    *v = KeyValueItem::Scalar(value.clone());
                }
            })
            .or_insert(KeyValueItem::Scalar(value));
        Ok(())
    }

    pub fn lrem(&mut self, key: &str, value: String) -> Result<i32, Box<dyn Error>> {
        let mut len: i32 = 0;
        self.items.entry(key.to_string()).and_modify(|v| {
            if let KeyValueItem::List(ref l) = v {
                let list: Vec<String> = l
                    .iter()
                    .filter(|i| **i != value)
                    .map(|v| v.into())
                    .collect();
                len = list.len() as _;
                *v = KeyValueItem::List(list);
            }
        });
        Ok(len)
    }

    pub fn sadd(&mut self, key: &str, value: String) -> Result<i32, Box<dyn Error>> {
        let mut len: i32 = 1;
        self.items
            .entry(key.to_string())
            .and_modify(|v| {
                if let KeyValueItem::Set(ref mut s) = v {
                    s.insert(value.clone());
                    len = s.len() as _;
                }
            })
            .or_insert_with(|| new_set(value));
        Ok(len)
    }

    pub fn srem(&mut self, key: &str, value: String) -> Result<i32, Box<dyn Error>> {
        let mut len: i32 = 0;
        self.items
            .entry(key.to_string())
            .and_modify(|v| {
                if let KeyValueItem::Set(ref mut s) = v {
                    s.remove(&value);
                    len = s.len() as _;
                }
            })
            .or_insert_with(|| KeyValueItem::Set(HashSet::new()));
        Ok(len)
    }

    pub fn sunion(&self, keys: Vec<String>) -> Result<Vec<String>, Box<dyn Error>> {
        let union = self
            .items
            .iter()
            .filter_map(|(k, v)| {
                if keys.contains(k) {
                    if let KeyValueItem::Set(s) = v {
                        Some(s.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .fold(HashSet::new(), |acc, x| acc.union(&x).cloned().collect());

        Ok(union.iter().cloned().collect())
    }

    pub fn sinter(&self, keys: Vec<String>) -> Result<Vec<String>, Box<dyn Error>> {
        let sets: Vec<HashSet<String>> = self
            .items
            .iter()
            .filter_map(|(k, v)| {
                if keys.contains(k) {
                    if let KeyValueItem::Set(s) = v {
                        Some(s.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();
        let set1 = &sets[0];
        let inter = set1
            .iter()
            .filter(|k| sets.as_slice().iter().all(|s| s.contains(*k)));
        Ok(inter.cloned().collect())
    }

    pub fn smembers(&self, key: String) -> Result<Vec<String>, Box<dyn Error>> {
        self.items.get(&key).map_or_else(
            || Ok(vec![]),
            |v| {
                if let KeyValueItem::Set(ref s) = v {
                    Ok(s.iter().cloned().collect())
                } else {
                    Err(format!("attempt to query non-set '{}'", key).into())
                }
            },
        )
    }
}

fn new_set(value: String) -> KeyValueItem {
    let mut x = HashSet::new();
    x.insert(value);
    KeyValueItem::Set(x)
}

#[cfg(test)]
mod test {
    use super::KeyValueStore;

    fn gen_store() -> KeyValueStore {
        let mut store = KeyValueStore::new();
        store.sadd("test", "bob".to_string()).unwrap();
        store.sadd("test", "alice".to_string()).unwrap();
        store.sadd("test", "dave".to_string()).unwrap();
        store.sadd("test2", "bob".to_string()).unwrap();
        store.sadd("test2", "dave".to_string()).unwrap();

        store.lpush("list1", "first".to_string()).unwrap();
        store.lpush("list1", "second".to_string()).unwrap();
        store.lpush("list1", "third".to_string()).unwrap();

        store.incr("counter", 5).unwrap();

        store.set("setkey", "setval".to_string()).unwrap();
        store
    }

    #[test]
    fn test_intersect() {
        let store = gen_store();

        let inter = store
            .sinter(vec!["test".to_string(), "test2".to_string()])
            .unwrap();
        assert!(inter.contains(&String::from("bob")));
        assert!(inter.contains(&String::from("dave")));
        assert_eq!(false, inter.contains(&String::from("alice")));
    }

    #[test]
    fn test_union() {
        let store = gen_store();

        let union = store
            .sunion(vec!["test".to_string(), "test2".to_string()])
            .unwrap();
        assert_eq!(3, union.len());
    }

    #[test]
    fn test_get_set() {
        let store = gen_store();

        assert_eq!("setval".to_string(), store.get("setkey").unwrap());
    }

    #[test]
    fn test_list() {
        let store = gen_store();
        assert_eq!(
            vec!["first", "second", "third"],
            store.lrange("list1", 0, 100).unwrap()
        );
    }

    #[test]
    fn test_incr() {
        let mut store = gen_store();

        let a = store.incr("counter", 1).unwrap();
        let b = store.incr("counter", 1).unwrap();
        let c = store.incr("counter", -3).unwrap();

        assert_eq!(a, 6);
        assert_eq!(b, 7);
        assert_eq!(c, 4);
    }

    #[test]
    fn test_exists_and_del() {
        let mut store = gen_store();

        store.set("thenumber", "42".to_string()).unwrap();
        assert!(store.exists("thenumber").unwrap());
        store.del("thenumber").unwrap();
        assert_eq!(false, store.exists("thenumber").unwrap());
        store.set("thenumber", "41".to_string()).unwrap();
        assert!(store.exists("thenumber").unwrap());
    }
}
