/// Exercise route registration and matching without a running host.
pub fn fuzz_route_matcher(entry: &str, host: &str, path: &str) {
    let Some(route) = parse_local_route(entry) else {
        return;
    };
    let router = DynamicRouter::default();
    router.register_routes("local", std::slice::from_ref(&route));
    let expected =
        IngressRoute::local(host, "").host == route.host && route.path_prefix.matches(path);
    assert_eq!(
        router
            .select_workload(host, path, RouteScope::Local)
            .is_ok(),
        expected
    );
    assert!(
        router
            .select_workload(host, path, RouteScope::Ingress)
            .is_err()
    );
    router.register_routes("ingress", &[IngressRoute::ingress(&route.host)]);
    assert_eq!(
        router
            .select_workload(host, path, RouteScope::Local)
            .is_ok(),
        expected
    );
    router.unregister_routes("local");
    assert!(
        router
            .select_workload(host, path, RouteScope::Local)
            .is_err()
    );
}
