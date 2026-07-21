//! PAC per-request routing for the app layer.

#[cfg(test)]
mod tests {
    use super::to_route_choice;
    use zicade_config::{AuthMode, FailPolicy, PacConfig, RoutingConfig, RoutingMode};
    use zicade_proxy::RouteChoice;
    use zicade_routing::{PacResult, RoutingError};

    fn routing_pac(fail: FailPolicy) -> RoutingConfig {
        let mut pac = PacConfig::default();
        pac.auth.mode = AuthMode::Negotiate;
        pac.fail_policy = fail;
        RoutingConfig {
            mode: RoutingMode::Pac,
            upstream: None,
            pac: Some(pac),
        }
    }

    #[test]
    fn proxy_result_builds_upstream_inheriting_pac_auth() {
        // LESSON-6: the PAC-selected upstream inherits routing.pac.auth and its
        // address is the RESOLVED proxy host:port.
        let routing = routing_pac(FailPolicy::Error);
        let choice = to_route_choice(
            Ok(PacResult::Proxy {
                host: "wp8080".to_owned(),
                port: 8080,
            }),
            &routing,
        )
        .expect("proxy result maps to a route choice");
        match choice {
            RouteChoice::Upstream(target) => {
                assert_eq!(target.addr, "wp8080:8080");
                assert_eq!(format!("{:?}", target.auth), "Negotiate");
            }
            RouteChoice::Direct => panic!("expected Upstream, got Direct"),
        }
    }

    #[test]
    fn direct_result_maps_to_direct() {
        let routing = routing_pac(FailPolicy::Error);
        let choice = to_route_choice(Ok(PacResult::Direct), &routing).unwrap();
        assert!(matches!(choice, RouteChoice::Direct));
    }

    #[test]
    fn error_with_fail_policy_direct_falls_back_to_direct() {
        let routing = routing_pac(FailPolicy::Direct);
        let choice =
            to_route_choice(Err(RoutingError::Backend("no PAC".to_owned())), &routing).unwrap();
        assert!(matches!(choice, RouteChoice::Direct));
    }

    #[test]
    fn error_with_fail_policy_error_propagates() {
        let routing = routing_pac(FailPolicy::Error);
        let result = to_route_choice(Err(RoutingError::Backend("no PAC".to_owned())), &routing);
        assert!(result.is_err(), "FailPolicy::Error must surface an error");
    }
}
