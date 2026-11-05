#![no_main]

use oci_client::Reference;
use wash_runtime::component_source::{ComponentSource, LoadedComponent};
use wash_runtime::oci::parse_reference;
use wash_runtime_fuzz::fuzz_target;

fuzz_target!(|data: &str| {
    let (reference, expected) = data.split_once('\n').unwrap_or((data, data));
    let _ = ComponentSource::from_reference(reference);
    if let Ok(parsed) = Reference::try_from(reference) {
        let normalized = parsed.to_string();
        // Normalization can produce a spelling the OCI parser rejects.
        if let Ok(reparsed) = Reference::try_from(normalized.as_str()) {
            assert_eq!(parsed, reparsed);
            // Pull and push validate with `parse_reference`; a reference it
            // accepts must stay accepted once normalized.
            if parse_reference(reference).is_ok() {
                assert!(parse_reference(&normalized).is_ok());
            }
        }
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
