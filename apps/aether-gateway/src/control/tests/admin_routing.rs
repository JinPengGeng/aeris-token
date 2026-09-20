use http::Uri;

use crate::handlers::shared::local_proxy_route_requires_buffered_body;

use super::{classify_control_route, headers, GatewayPublicRequestContext};

#[test]
fn classifies_admin_routing_group_routes_as_admin_proxy_route() {
    let headers = headers(&[]);

    let list_uri: Uri = "/api/admin/routing/groups"
        .parse()
        .expect("uri should parse");
    let list = classify_control_route(&http::Method::GET, &list_uri, &headers)
        .expect("route should classify");
    assert_eq!(list.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(
        list.route_family.as_deref(),
        Some("routing_profiles_manage")
    );
    assert_eq!(list.route_kind.as_deref(), Some("list_groups"));
    assert_eq!(
        list.auth_endpoint_signature.as_deref(),
        Some("admin:routing_profiles")
    );

    let create_uri: Uri = "/api/admin/routing/groups"
        .parse()
        .expect("uri should parse");
    let create = classify_control_route(&http::Method::POST, &create_uri, &headers)
        .expect("route should classify");
    assert_eq!(
        create.route_family.as_deref(),
        Some("routing_profiles_manage")
    );
    assert_eq!(create.route_kind.as_deref(), Some("create_group"));

    let update_uri: Uri = "/api/admin/routing/groups/group-1"
        .parse()
        .expect("uri should parse");
    let update = classify_control_route(&http::Method::PATCH, &update_uri, &headers)
        .expect("route should classify");
    assert_eq!(
        update.route_family.as_deref(),
        Some("routing_profiles_manage")
    );
    assert_eq!(update.route_kind.as_deref(), Some("update_group"));

    let dry_run_uri: Uri = "/api/admin/routing/groups/group-1/dry-run"
        .parse()
        .expect("uri should parse");
    let dry_run = classify_control_route(&http::Method::POST, &dry_run_uri, &headers)
        .expect("route should classify");
    assert_eq!(
        dry_run.route_family.as_deref(),
        Some("routing_profiles_manage")
    );
    assert_eq!(dry_run.route_kind.as_deref(), Some("dry_run_group"));
}

#[test]
fn admin_v1_routes_classify_against_the_canonical_admin_path() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/v1/routing/groups?limit=10"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::GET, &uri, &headers)
        .expect("versioned admin route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(
        decision.route_family.as_deref(),
        Some("routing_profiles_manage")
    );
    assert_eq!(decision.route_kind.as_deref(), Some("list_groups"));
    assert_eq!(decision.public_path, "/api/admin/routing/groups");
    assert_eq!(uri.path(), "/api/admin/v1/routing/groups");
    assert_eq!(uri.query(), Some("limit=10"));
}

#[test]
fn admin_v1_prefix_does_not_match_similar_versions() {
    let headers = headers(&[]);
    for path in [
        "/api/admin/v10/routing/groups",
        "/api/admin/v1x/routing/groups",
    ] {
        let uri: Uri = path.parse().expect("uri should parse");
        assert!(
            classify_control_route(&http::Method::GET, &uri, &headers).is_none(),
            "{path} must not be treated as an admin v1 alias"
        );
    }
}

#[test]
fn admin_routing_write_routes_buffer_request_body() {
    let headers = headers(&[]);
    let routes = [
        (http::Method::POST, "/api/admin/routing/groups"),
        (http::Method::PATCH, "/api/admin/routing/groups/group-1"),
        (
            http::Method::POST,
            "/api/admin/routing/groups/group-1/dry-run",
        ),
        (http::Method::POST, "/api/admin/routing/bindings"),
        (http::Method::PATCH, "/api/admin/routing/bindings/binding-1"),
    ];

    for (method, path) in routes {
        let uri: Uri = path.parse().expect("uri should parse");
        let decision =
            classify_control_route(&method, &uri, &headers).expect("route should classify");
        let context = GatewayPublicRequestContext::from_request_parts(
            "trace-routing-write",
            &method,
            &uri,
            &headers,
            Some(decision),
        );

        assert!(
            local_proxy_route_requires_buffered_body(&context),
            "{method} {path} should buffer request body"
        );
    }
}
