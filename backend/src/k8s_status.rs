//! Pure functions deriving live status and endpoint from k8s objects.
//!
//! No I/O — handlers fetch the StatefulSet/Pod/Service first, then call
//! these helpers. Keeping them pure makes them easy to unit-test against
//! hand-built `Default::default()` k8s structs.

use k8s_openapi::api::core::v1::{Pod, Service};

use crate::k8s::{Endpoint, ServerStatus};

/// Minecraft TCP port used by the itzg image.
pub const MC_PORT: u16 = 25_565;

/// RCON TCP port. Internal-only: published on the per-server headless
/// Service, never on the public Service.
pub const RCON_PORT: u16 = 25_575;

/// Container `waiting` reasons that signal a terminal pod failure.
const ERROR_REASONS: &[&str] = &[
    "CrashLoopBackOff",
    "ImagePullBackOff",
    "ErrImagePull",
    "CreateContainerConfigError",
    "RunContainerError",
];

/// How long a pod can sit `ContainersNotReady` before we surface it as
/// `Error`. Tuned to the slowest expected boot (mod-heavy `CurseForge`
/// pack on a cold node); past this, it's almost always a real failure
/// rather than a long boot.
const CONTAINERS_NOT_READY_GRACE_SECS: i64 = 60;

/// Derives the live [`ServerStatus`] from the `StatefulSet`'s replicas/ready
/// counts plus the optional Pod.
///
/// Truth table (per spec §2.4 + M2 enrichment):
/// - `replicas <= 0` and no pod / pod gone   → `Stopped`
/// - `replicas <= 0` and pod terminating     → `Stopping`
/// - `replicas >= 1` and `ready >= 1`        → `Running`
/// - `replicas >= 1` and pod in error state  → `Error`
/// - `replicas >= 1` and pod not ready       → `Starting`
#[must_use]
pub fn derive_status(replicas: i32, ready_replicas: i32, pod: Option<&Pod>) -> ServerStatus {
    if replicas <= 0 {
        // Pod with a `deletionTimestamp` set is mid-termination.
        if let Some(p) = pod
            && p.metadata.deletion_timestamp.is_some()
        {
            return ServerStatus::Stopping;
        }
        return ServerStatus::Stopped;
    }

    if ready_replicas >= 1 {
        return ServerStatus::Running;
    }

    if let Some(p) = pod
        && pod_in_error_state(p)
    {
        return ServerStatus::Error;
    }

    ServerStatus::Starting
}

/// Returns `true` if the pod is in a terminal failure mode anvil should
/// surface as `Error`:
///
/// - any container is waiting with one of the [`ERROR_REASONS`];
/// - `PodScheduled = False` with reason `Unschedulable` (no node has
///   the resources / volumes / taints to host the pod);
/// - `ContainersReady = False` for longer than
///   [`CONTAINERS_NOT_READY_GRACE_SECS`] (boot has stalled).
///
/// A single restart no longer counts as `Error` — Minecraft servers
/// occasionally restart cleanly during world-load and the panel
/// shouldn't flag that. We rely on `ERROR_REASONS` to catch the real
/// crash loop; that reason is set by kubelet on the second restart.
fn pod_in_error_state(pod: &Pod) -> bool {
    let Some(status) = pod.status.as_ref() else {
        return false;
    };

    if let Some(conditions) = status.conditions.as_ref() {
        for c in conditions {
            if c.type_ == "PodScheduled"
                && c.status == "False"
                && c.reason.as_deref() == Some("Unschedulable")
            {
                return true;
            }
            if c.type_ == "ContainersReady" && c.status == "False" {
                // `Time` wraps `k8s_openapi::jiff::Timestamp`. Use the
                // re-exported jiff so we don't need it as a runtime
                // dependency on its own.
                let stuck_for = c.last_transition_time.as_ref().map_or(0, |t| {
                    k8s_openapi::jiff::Timestamp::now().as_second() - t.0.as_second()
                });
                if stuck_for >= CONTAINERS_NOT_READY_GRACE_SECS {
                    return true;
                }
            }
        }
    }

    let Some(statuses) = status.container_statuses.as_ref() else {
        return false;
    };
    statuses.iter().any(|cs| {
        cs.state
            .as_ref()
            .and_then(|st| st.waiting.as_ref())
            .and_then(|w| w.reason.as_deref())
            .is_some_and(|r| ERROR_REASONS.contains(&r))
    })
}

/// Derives the connection [`Endpoint`] for a managed server.
///
/// - `loadbalancer` — reads `svc.status.loadBalancer.ingress[0].ip`. Returns
///   `None` while the LB IP is pending.
/// - `nodeport`     — `node_host:<assigned-nodePort>`. Returns `None` if
///   either is missing.
/// - `clusterip`    — `<svc_name>.<namespace>.svc.cluster.local:25565`.
///   Does not require the live Service object.
///
/// Unknown modes return `None`.
#[must_use]
pub fn derive_endpoint(
    svc: Option<&Service>,
    exposure_mode: &str,
    node_host: &str,
    svc_name: &str,
    namespace: &str,
) -> Option<Endpoint> {
    match exposure_mode {
        "loadbalancer" => {
            let ingress = svc?
                .status
                .as_ref()?
                .load_balancer
                .as_ref()?
                .ingress
                .as_ref()?
                .first()?;
            // Some LB providers (most cloud LBs, MetalLB with
            // hostname-only configs) populate `hostname` instead of
            // `ip`. Fall back so the panel surfaces the address either
            // way.
            let host = ingress.ip.clone().or_else(|| ingress.hostname.clone())?;
            Some(Endpoint {
                host,
                port: MC_PORT,
            })
        }
        "nodeport" => {
            if node_host.is_empty() {
                return None;
            }
            let np = svc?.spec.as_ref()?.ports.as_ref()?.first()?.node_port?;
            // node_port is i32 in k8s; clamp into u16 (k8s itself enforces 30000-32767).
            let port = u16::try_from(np).ok()?;
            Some(Endpoint {
                host: node_host.to_owned(),
                port,
            })
        }
        "clusterip" => Some(Endpoint {
            host: format!("{svc_name}.{namespace}.svc.cluster.local"),
            port: MC_PORT,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::core::v1::{
        ContainerState, ContainerStateWaiting, ContainerStatus, LoadBalancerIngress,
        LoadBalancerStatus, PodStatus, ServicePort, ServiceSpec, ServiceStatus,
    };
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;

    fn make_pod_with_waiting(reason: &str) -> Pod {
        Pod {
            status: Some(PodStatus {
                container_statuses: Some(vec![ContainerStatus {
                    name: "mc".to_owned(),
                    state: Some(ContainerState {
                        waiting: Some(ContainerStateWaiting {
                            reason: Some(reason.to_owned()),
                            ..ContainerStateWaiting::default()
                        }),
                        ..ContainerState::default()
                    }),
                    ..ContainerStatus::default()
                }]),
                ..PodStatus::default()
            }),
            ..Pod::default()
        }
    }

    fn make_pod_with_restart_count(restart_count: i32, ready: bool) -> Pod {
        Pod {
            status: Some(PodStatus {
                container_statuses: Some(vec![ContainerStatus {
                    name: "mc".to_owned(),
                    restart_count,
                    ready,
                    state: Some(ContainerState::default()),
                    ..ContainerStatus::default()
                }]),
                ..PodStatus::default()
            }),
            ..Pod::default()
        }
    }

    #[test]
    fn replicas_zero_no_pod_is_stopped() {
        assert_eq!(derive_status(0, 0, None), ServerStatus::Stopped);
    }

    #[test]
    fn replicas_zero_pod_terminating_is_stopping() {
        let mut pod = Pod::default();
        pod.metadata.deletion_timestamp = Some(Time(jiff::Timestamp::now()));
        assert_eq!(derive_status(0, 0, Some(&pod)), ServerStatus::Stopping);
    }

    #[test]
    fn replicas_zero_pod_present_no_terminate_is_stopped() {
        // Pod still exists but the StatefulSet says replicas=0 — race window
        // we treat as Stopped; Stopping requires the deletionTimestamp.
        let pod = Pod::default();
        assert_eq!(derive_status(0, 0, Some(&pod)), ServerStatus::Stopped);
    }

    #[test]
    fn replicas_one_ready_is_running() {
        assert_eq!(derive_status(1, 1, None), ServerStatus::Running);
    }

    #[test]
    fn replicas_one_unready_no_pod_is_starting() {
        assert_eq!(derive_status(1, 0, None), ServerStatus::Starting);
    }

    #[test]
    fn replicas_one_pod_crashloop_is_error() {
        let pod = make_pod_with_waiting("CrashLoopBackOff");
        assert_eq!(derive_status(1, 0, Some(&pod)), ServerStatus::Error);
    }

    #[test]
    fn replicas_one_pod_image_pull_error_is_error() {
        let pod = make_pod_with_waiting("ImagePullBackOff");
        assert_eq!(derive_status(1, 0, Some(&pod)), ServerStatus::Error);
    }

    #[test]
    fn replicas_one_pod_pending_is_starting() {
        let pod = make_pod_with_waiting("PodInitializing");
        assert_eq!(derive_status(1, 0, Some(&pod)), ServerStatus::Starting);
    }

    #[test]
    fn replicas_one_restart_count_unready_is_starting() {
        // Single restart without a terminal waiting reason is no longer
        // treated as Error — MC sometimes restarts during world load
        // and a clean restart shouldn't trip the panel.
        let pod = make_pod_with_restart_count(1, false);
        assert_eq!(derive_status(1, 0, Some(&pod)), ServerStatus::Starting);
    }

    #[test]
    fn replicas_one_restart_count_ready_is_running() {
        // ready_replicas short-circuits to Running before pod_in_error_state runs.
        let pod = make_pod_with_restart_count(2, true);
        assert_eq!(derive_status(1, 1, Some(&pod)), ServerStatus::Running);
    }

    #[test]
    fn replicas_one_zero_restarts_no_error_reason_is_starting() {
        let pod = make_pod_with_restart_count(0, false);
        assert_eq!(derive_status(1, 0, Some(&pod)), ServerStatus::Starting);
    }

    #[test]
    fn endpoint_loadbalancer_returns_ingress_ip() {
        let svc = Service {
            spec: Some(ServiceSpec {
                type_: Some("LoadBalancer".to_owned()),
                ..ServiceSpec::default()
            }),
            status: Some(ServiceStatus {
                load_balancer: Some(LoadBalancerStatus {
                    ingress: Some(vec![LoadBalancerIngress {
                        ip: Some("10.0.0.5".to_owned()),
                        ..LoadBalancerIngress::default()
                    }]),
                }),
                ..ServiceStatus::default()
            }),
            ..Service::default()
        };
        let ep = derive_endpoint(Some(&svc), "loadbalancer", "", "mc-x", "mc").expect("endpoint");
        assert_eq!(ep.host, "10.0.0.5");
        assert_eq!(ep.port, MC_PORT);
    }

    #[test]
    fn endpoint_loadbalancer_pending_returns_none() {
        let svc = Service {
            spec: Some(ServiceSpec {
                type_: Some("LoadBalancer".to_owned()),
                ..ServiceSpec::default()
            }),
            status: Some(ServiceStatus {
                load_balancer: Some(LoadBalancerStatus { ingress: None }),
                ..ServiceStatus::default()
            }),
            ..Service::default()
        };
        assert!(derive_endpoint(Some(&svc), "loadbalancer", "", "mc-x", "mc").is_none());
    }

    #[test]
    fn endpoint_nodeport_uses_node_host_and_assigned_port() {
        let svc = Service {
            spec: Some(ServiceSpec {
                type_: Some("NodePort".to_owned()),
                ports: Some(vec![ServicePort {
                    port: 25_565,
                    node_port: Some(30_005),
                    ..ServicePort::default()
                }]),
                ..ServiceSpec::default()
            }),
            ..Service::default()
        };
        let ep =
            derive_endpoint(Some(&svc), "nodeport", "node.local", "mc-x", "mc").expect("endpoint");
        assert_eq!(ep.host, "node.local");
        assert_eq!(ep.port, 30_005);
    }

    #[test]
    fn endpoint_nodeport_no_node_host_is_none() {
        let svc = Service {
            spec: Some(ServiceSpec {
                type_: Some("NodePort".to_owned()),
                ports: Some(vec![ServicePort {
                    port: 25_565,
                    node_port: Some(30_005),
                    ..ServicePort::default()
                }]),
                ..ServiceSpec::default()
            }),
            ..Service::default()
        };
        assert!(derive_endpoint(Some(&svc), "nodeport", "", "mc-x", "mc").is_none());
    }

    #[test]
    fn endpoint_clusterip_returns_dns() {
        let ep = derive_endpoint(None, "clusterip", "", "mc-abc", "mc").expect("endpoint");
        assert_eq!(ep.host, "mc-abc.mc.svc.cluster.local");
        assert_eq!(ep.port, MC_PORT);
    }

    #[test]
    fn endpoint_unknown_mode_is_none() {
        assert!(derive_endpoint(None, "garbage", "", "mc-x", "mc").is_none());
    }
}
