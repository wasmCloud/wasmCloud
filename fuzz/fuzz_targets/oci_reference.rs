#![no_main]

use libfuzzer_sys::fuzz_target;
use oci_client::Reference;
use wash_runtime::component_source::{ComponentSource, LoadedComponent};

fuzz_target!(|data: &str| {
    let (reference, expected) = data.split_once('\n').unwrap_or((data, data));
    let _ = ComponentSource::from_reference(reference);
    if let Ok(parsed) = Reference::try_from(reference) {
        let reparsed = Reference::try_from(parsed.to_string().as_str()).unwrap();
        assert_eq!(parsed, reparsed);
        let component = LoadedComponent {
            bytes: Default::default(),
            digest: parsed.digest().map(str::to_owned),
        };
        assert_eq!(
            component.verify_digest(expected).is_ok(),
            parsed.digest() == Some(expected)
        );
    }
    // Digest grammar is validated as part of an OCI reference.
    let _ = Reference::try_from(format!("registry.invalid/workload@{expected}").as_str());
});
