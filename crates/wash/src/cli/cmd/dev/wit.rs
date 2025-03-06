use std::collections::HashSet;

use anyhow::{bail, Context as _, Result};
use wit_parser::{Resolve, WorldId};

use crate::lib::parser::ProjectConfig;

use super::deps::DependencySpec;

/// Parse Build a [`wit_parser::Resolve`] from a provided component
pub fn parse_component_wit(component: &[u8]) -> Result<(Resolve, WorldId)> {
    match wit_parser::decoding::decode(component).context("failed to decode WIT component")? {
        wit_parser::decoding::DecodedWasm::Component(resolve, world) => Ok((resolve, world)),
        wit_parser::decoding::DecodedWasm::WitPackage(..) => {
            bail!("binary-encoded WIT packages not currently supported for wash dev")
        }
    }
}

/// Parse Build a [`wit_parser::Resolve`] from a provided directory
/// and select a given world
pub fn parse_project_wit(project_cfg: &ProjectConfig) -> Result<(Resolve, WorldId)> {
    let project_dir = &project_cfg.common.project_dir;
    let wit_dir = project_dir.join("wit");
    let world = project_cfg.project_type.wit_world();

    // Resolve the WIT directory packages & worlds
    let mut resolve = wit_parser::Resolve::default();
    let (package_id, _paths) = resolve
        .push_dir(wit_dir)
        .with_context(|| format!("failed to add WIT directory @ [{}]", project_dir.display()))?;

    // Select the target world that was specified by the user
    let world_id = resolve
        .select_world(package_id, world.as_deref())
        .context("failed to select world from built resolver")?;

    Ok((resolve, world_id))
}

/// Resolve the dependencies of a given WIT world that map to WADM components
///
/// Normally, this means converting imports that the component depends on to
/// components that can be run on the lattice.
pub fn discover_dependencies_from_wit(
    resolve: Resolve,
    world_id: WorldId,
) -> Result<Vec<DependencySpec>> {
    let mut deps: Vec<DependencySpec> = Vec::new();
    let world = resolve
        .worlds
        .get(world_id)
        .context("selected WIT world is missing")?;
    // Process imports
    for (_key, item) in &world.imports {
        if let wit_parser::WorldItem::Interface { id, .. } = item {
            let iface = resolve
                .interfaces
                .get(*id)
                .context("unexpectedly missing iface")?;
            let pkg = resolve
                .packages
                .get(iface.package.context("iface missing package")?)
                .context("failed to find package")?;
            let interface_name = iface.name.as_ref().context("interface missing name")?;
            let iface_name = &format!("{}:{}/{interface_name}", pkg.name.namespace, pkg.name.name,);

            if let Some(new_dep) = DependencySpec::from_wit_import_iface(iface_name) {
                // If the dependency already exists for a different interface, add the interface to the existing dependency
                if let Some(DependencySpec::Exports(ref mut dep)) = deps.iter_mut().find(|d| {
                    d.wit().namespace == pkg.name.namespace && d.wit().package == pkg.name.name
                }) {
                    if let Some(ref mut interfaces) = dep.wit.interfaces {
                        interfaces.insert(interface_name.to_string());
                    } else {
                        dep.wit.interfaces = Some(HashSet::from([iface_name.to_string()]));
                    }
                } else {
                    deps.push(new_dep);
                }
            }
        }
    }
    // Process exports
    for (_key, item) in &world.exports {
        if let wit_parser::WorldItem::Interface { id, .. } = item {
            let iface = resolve
                .interfaces
                .get(*id)
                .context("unexpectedly missing iface")?;
            let pkg = resolve
                .packages
                .get(iface.package.context("iface missing package")?)
                .context("failed to find package")?;
            let interface_name = iface.name.as_ref().context("interface missing name")?;
            let iface_name = &format!("{}:{}/{interface_name}", pkg.name.namespace, pkg.name.name,);
            if let Some(new_dep) = DependencySpec::from_wit_export_iface(iface_name) {
                // If the dependency already exists for a different interface, add the interface to the existing dependency
                if let Some(DependencySpec::Imports(ref mut dep)) = deps.iter_mut().find(|d| {
                    d.wit().namespace == pkg.name.namespace && d.wit().package == pkg.name.name
                }) {
                    if let Some(ref mut interfaces) = dep.wit.interfaces {
                        // SAFETY: we already checked at `iface.name`
                        interfaces.insert(interface_name.to_string());
                    } else {
                        dep.wit.interfaces = Some(HashSet::from([iface_name.to_string()]));
                    }
                } else {
                    deps.push(new_dep);
                }
            }
        }
    }

    Ok(deps)
}
