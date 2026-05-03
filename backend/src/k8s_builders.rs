//! Pure constructors for the per-server k8s objects.
//!
//! These functions take a [`BuildParams`] and return typed kube-rs
//! objects ready to hand to `Api::create()`. No I/O, no allocation
//! beyond what the resulting structs need. Unit tests verify the
//! resulting shapes — there is no integration test against a kube
//! API in M2.

use std::collections::BTreeMap;

use k8s_openapi::ByteString;
use k8s_openapi::api::apps::v1::{StatefulSet, StatefulSetSpec};
use k8s_openapi::api::core::v1::{
    Container, ContainerPort, EnvVar, PersistentVolumeClaim, PersistentVolumeClaimSpec, PodSpec,
    PodTemplateSpec, ResourceRequirements, Secret, Service, ServicePort, ServiceSpec, VolumeMount,
    VolumeResourceRequirements,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use rand::RngExt as _;
use rand::distr::Alphanumeric;

use crate::k8s::{
    ANNOTATION_CREATED_AT, ANNOTATION_MC_VERSION, ANNOTATION_MEMORY_MI, ANNOTATION_SERVER_NAME,
    LABEL_SERVER, MANAGED_BY_LABEL, MANAGED_BY_VALUE,
};
use crate::k8s_status::{MC_PORT, RCON_PORT};

/// Length of the generated RCON password.
const RCON_PASSWORD_LEN: usize = 24;

/// Inputs needed to construct the StatefulSet/Service/Secret triple.
///
/// M5: `image`, `command`, and `extra_env` are now provider-supplied so the
/// `CurseForge` path can launch its own bash entrypoint with custom JVM env.
#[derive(Debug, Clone)]
pub struct BuildParams<'a> {
    /// Server UUID; used as the resource-name suffix `mc-<id>`.
    pub id: &'a str,
    /// User-facing name (label / annotation only).
    pub name: &'a str,
    /// Namespace where managed resources live.
    pub namespace: &'a str,
    /// Minecraft version snapshotted at create time. Stored as an annotation.
    pub mc_version: &'a str,
    /// Memory budget in MiB. Becomes the k8s resource limit; provider env
    /// (`MEMORY=…M` for vanilla, `JAVA_TOOL_OPTIONS=-Xmx…m` for CF) lives
    /// in `extra_env`.
    pub memory_mi: i64,
    /// Server type — `vanilla` | `curseforge` (`servers.source_kind`).
    pub server_type: &'a str,
    /// Container image (e.g. `itzg/minecraft-server:java21`,
    /// `eclipse-temurin:21-jdk`). Comes from `ModpackProvider::pod_image`.
    pub image: &'a str,
    /// Container command override; `None` lets the image entrypoint run.
    pub command: Option<&'a [String]>,
    /// Provider-supplied env vars (already includes RCON for vanilla).
    pub extra_env: &'a [EnvVar],
    /// Service exposure mode (`loadbalancer` | `nodeport` | `clusterip`).
    pub exposure_mode: &'a str,
    /// PVC `storageClassName`. `None` => omit field, k8s uses cluster default.
    pub storage_class: Option<&'a str>,
    /// PVC size in GiB.
    pub storage_size_gi: i64,
    /// Assigned `NodePort`. Must be `Some` when `exposure_mode == "nodeport"`.
    pub nodeport: Option<i32>,
    /// Unix-second creation timestamp.
    pub created_at: i64,
}

/// Returns the standard managed-by + per-server label set.
fn server_labels(id: &str) -> BTreeMap<String, String> {
    let mut labels = BTreeMap::new();
    labels.insert(MANAGED_BY_LABEL.to_owned(), MANAGED_BY_VALUE.to_owned());
    labels.insert(LABEL_SERVER.to_owned(), id.to_owned());
    labels
}

/// Returns the standard annotation set on the `StatefulSet`.
fn server_annotations(p: &BuildParams<'_>) -> BTreeMap<String, String> {
    let mut a = BTreeMap::new();
    a.insert(ANNOTATION_SERVER_NAME.to_owned(), p.name.to_owned());
    a.insert(ANNOTATION_CREATED_AT.to_owned(), p.created_at.to_string());
    a.insert(ANNOTATION_MC_VERSION.to_owned(), p.mc_version.to_owned());
    a.insert(ANNOTATION_MEMORY_MI.to_owned(), p.memory_mi.to_string());
    a
}

/// Builds the `StatefulSet` for a managed server (replicas=0, single
/// container running the provider-supplied image).
#[must_use]
pub fn build_statefulset(p: &BuildParams<'_>) -> StatefulSet {
    let resource_name = format!("mc-{}", p.id);
    let labels = server_labels(p.id);
    let annotations = server_annotations(p);

    let resources = pod_resources(p.memory_mi);

    let container = Container {
        name: "mc".to_owned(),
        image: Some(p.image.to_owned()),
        command: p.command.map(<[String]>::to_vec),
        // The data volume mounts at /data; CurseForge packs unpack here and
        // their startserver.sh expects /data as the working directory.
        working_dir: Some("/data".to_owned()),
        env: Some(p.extra_env.to_vec()),
        ports: Some(vec![
            ContainerPort {
                container_port: i32::from(MC_PORT),
                name: Some("mc".to_owned()),
                protocol: Some("TCP".to_owned()),
                ..ContainerPort::default()
            },
            ContainerPort {
                container_port: i32::from(RCON_PORT),
                name: Some("rcon".to_owned()),
                protocol: Some("TCP".to_owned()),
                ..ContainerPort::default()
            },
        ]),
        resources: Some(resources),
        volume_mounts: Some(vec![VolumeMount {
            name: "data".to_owned(),
            mount_path: "/data".to_owned(),
            ..VolumeMount::default()
        }]),
        ..Container::default()
    };

    let pod_template = PodTemplateSpec {
        metadata: Some(ObjectMeta {
            labels: Some(labels.clone()),
            ..ObjectMeta::default()
        }),
        spec: Some(PodSpec {
            containers: vec![container],
            ..PodSpec::default()
        }),
    };

    let storage_request: BTreeMap<String, Quantity> = std::iter::once((
        "storage".to_owned(),
        Quantity(format!("{}Gi", p.storage_size_gi)),
    ))
    .collect();

    let pvc_template = PersistentVolumeClaim {
        metadata: ObjectMeta {
            name: Some("data".to_owned()),
            ..ObjectMeta::default()
        },
        spec: Some(PersistentVolumeClaimSpec {
            access_modes: Some(vec!["ReadWriteOnce".to_owned()]),
            storage_class_name: p.storage_class.map(str::to_owned),
            resources: Some(VolumeResourceRequirements {
                requests: Some(storage_request),
                ..VolumeResourceRequirements::default()
            }),
            ..PersistentVolumeClaimSpec::default()
        }),
        status: None,
    };

    let spec = StatefulSetSpec {
        replicas: Some(0),
        service_name: Some(format!("{resource_name}-headless")),
        selector: LabelSelector {
            match_labels: Some(labels.clone()),
            ..LabelSelector::default()
        },
        template: pod_template,
        volume_claim_templates: Some(vec![pvc_template]),
        ..StatefulSetSpec::default()
    };

    StatefulSet {
        metadata: ObjectMeta {
            name: Some(resource_name),
            namespace: Some(p.namespace.to_owned()),
            labels: Some(labels),
            annotations: Some(annotations),
            ..ObjectMeta::default()
        },
        spec: Some(spec),
        status: None,
    }
}

/// Builds the Service for a managed server. Type comes from
/// `exposure_mode`; for `NodePort` the assigned port is set from
/// `params.nodeport`.
///
/// # Panics
///
/// Panics if `exposure_mode == "nodeport"` and `nodeport` is `None`.
/// The create handler must always pre-allocate the port.
#[must_use]
pub fn build_service(p: &BuildParams<'_>) -> Service {
    let resource_name = format!("mc-{}", p.id);
    let labels = server_labels(p.id);

    let svc_type = match p.exposure_mode {
        "loadbalancer" => "LoadBalancer",
        "nodeport" => "NodePort",
        // ClusterIP is the safe default if validation has somehow let
        // an unknown value through.
        _ => "ClusterIP",
    };

    let mut port = ServicePort {
        port: i32::from(MC_PORT),
        target_port: Some(IntOrString::Int(i32::from(MC_PORT))),
        protocol: Some("TCP".to_owned()),
        name: Some("mc".to_owned()),
        ..ServicePort::default()
    };
    if p.exposure_mode == "nodeport" {
        port.node_port = Some(
            p.nodeport
                .expect("create handler must pre-assign a NodePort before calling build_service"),
        );
    }

    Service {
        metadata: ObjectMeta {
            name: Some(resource_name),
            namespace: Some(p.namespace.to_owned()),
            labels: Some(labels.clone()),
            ..ObjectMeta::default()
        },
        spec: Some(ServiceSpec {
            type_: Some(svc_type.to_owned()),
            selector: Some(labels),
            ports: Some(vec![port]),
            ..ServiceSpec::default()
        }),
        status: None,
    }
}

/// Builds the headless Service that gives the `StatefulSet` pod stable
/// per-pod DNS at `<sts>-0.<svc>.<ns>.svc:25575`.
///
/// Used exclusively for in-cluster RCON access. `cluster_ip = None` makes
/// the Service headless; `publish_not_ready_addresses = true` lets DNS
/// resolve while the pod is still becoming Ready (the RCON handler still
/// gates on `Running`, but this avoids transient `NXDOMAIN` flakes during
/// boot).
#[must_use]
pub fn build_headless_service(p: &BuildParams<'_>) -> Service {
    let resource_name = format!("mc-{}-headless", p.id);
    let labels = server_labels(p.id);
    Service {
        metadata: ObjectMeta {
            name: Some(resource_name),
            namespace: Some(p.namespace.to_owned()),
            labels: Some(labels.clone()),
            ..ObjectMeta::default()
        },
        spec: Some(ServiceSpec {
            cluster_ip: Some("None".to_owned()),
            publish_not_ready_addresses: Some(true),
            selector: Some(labels),
            ports: Some(vec![ServicePort {
                port: i32::from(RCON_PORT),
                target_port: Some(IntOrString::Int(i32::from(RCON_PORT))),
                protocol: Some("TCP".to_owned()),
                name: Some("rcon".to_owned()),
                ..ServicePort::default()
            }]),
            ..ServiceSpec::default()
        }),
        status: None,
    }
}

/// Builds the Secret holding the RCON password. The `StatefulSet` env var
/// `RCON_PASSWORD` references this Secret via `secretKeyRef`.
#[must_use]
pub fn build_rcon_secret(id: &str, namespace: &str, password: &str) -> Secret {
    let mut data: BTreeMap<String, ByteString> = BTreeMap::new();
    data.insert(
        "password".to_owned(),
        ByteString(password.as_bytes().to_vec()),
    );
    Secret {
        metadata: ObjectMeta {
            name: Some(format!("mc-{id}-rcon")),
            namespace: Some(namespace.to_owned()),
            labels: Some(server_labels(id)),
            ..ObjectMeta::default()
        },
        data: Some(data),
        type_: Some("Opaque".to_owned()),
        ..Secret::default()
    }
}

/// Generates a 24-char alphanumeric password suitable for the RCON
/// secret. Cryptographically random via `rand::rng()`.
#[must_use]
pub fn rcon_password() -> String {
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(RCON_PASSWORD_LEN)
        .map(char::from)
        .collect()
}

/// Returns CPU and memory limits for the Minecraft container.
///
/// Limits only — no requests. The homelab cluster is intentionally
/// overprovisioned, so binding scheduler-visible requests to the
/// per-server budget would force the operator to right-size every
/// server before deploying. Limits still cap runaway containers.
fn pod_resources(memory_mi: i64) -> ResourceRequirements {
    let mut limits: BTreeMap<String, Quantity> = BTreeMap::new();
    limits.insert("memory".to_owned(), Quantity(format!("{memory_mi}Mi")));
    limits.insert("cpu".to_owned(), Quantity("2000m".to_owned()));
    ResourceRequirements {
        requests: None,
        limits: Some(limits),
        claims: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modpack::VanillaProvider;
    use std::sync::OnceLock;

    static VANILLA_ENV: OnceLock<Vec<EnvVar>> = OnceLock::new();

    fn vanilla_env_for_test() -> &'static [EnvVar] {
        VANILLA_ENV.get_or_init(|| VanillaProvider::build_env("abcd1234", "1.21.4", 4096))
    }

    fn params() -> BuildParams<'static> {
        BuildParams {
            id: "abcd1234",
            name: "smp",
            namespace: "mc",
            mc_version: "1.21.4",
            memory_mi: 4096,
            server_type: "vanilla",
            image: "itzg/minecraft-server:java21",
            command: None,
            extra_env: vanilla_env_for_test(),
            exposure_mode: "loadbalancer",
            storage_class: Some("tank"),
            storage_size_gi: 10,
            nodeport: None,
            created_at: 1_700_000_000,
        }
    }

    #[test]
    fn statefulset_name_and_namespace() {
        let sts = build_statefulset(&params());
        assert_eq!(sts.metadata.name.as_deref(), Some("mc-abcd1234"));
        assert_eq!(sts.metadata.namespace.as_deref(), Some("mc"));
    }

    #[test]
    fn statefulset_replicas_zero() {
        let sts = build_statefulset(&params());
        assert_eq!(sts.spec.as_ref().unwrap().replicas, Some(0));
    }

    #[test]
    fn statefulset_managed_label_and_server_label() {
        let sts = build_statefulset(&params());
        let labels = sts.metadata.labels.as_ref().unwrap();
        assert_eq!(
            labels.get("app.anvil.io/managed-by").map(String::as_str),
            Some("anvil")
        );
        assert_eq!(
            labels.get("app.anvil.io/server").map(String::as_str),
            Some("abcd1234")
        );
    }

    #[test]
    fn statefulset_annotations_capture_metadata() {
        let sts = build_statefulset(&params());
        let a = sts.metadata.annotations.as_ref().unwrap();
        assert_eq!(
            a.get(ANNOTATION_SERVER_NAME).map(String::as_str),
            Some("smp")
        );
        assert_eq!(
            a.get(ANNOTATION_MC_VERSION).map(String::as_str),
            Some("1.21.4")
        );
        assert_eq!(
            a.get(ANNOTATION_MEMORY_MI).map(String::as_str),
            Some("4096")
        );
        assert_eq!(
            a.get(ANNOTATION_CREATED_AT).map(String::as_str),
            Some("1700000000")
        );
    }

    #[test]
    fn statefulset_memory_env_in_jvm_format() {
        let sts = build_statefulset(&params());
        let env = sts
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers[0]
            .env
            .as_ref()
            .unwrap();
        let mem = env.iter().find(|e| e.name == "MEMORY").unwrap();
        assert_eq!(mem.value.as_deref(), Some("4096M"));
    }

    #[test]
    fn statefulset_rcon_password_envvar_uses_secret_ref() {
        let sts = build_statefulset(&params());
        let env = sts
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers[0]
            .env
            .as_ref()
            .unwrap();
        let rcon = env.iter().find(|e| e.name == "RCON_PASSWORD").unwrap();
        assert!(rcon.value.is_none());
        let sk = rcon
            .value_from
            .as_ref()
            .unwrap()
            .secret_key_ref
            .as_ref()
            .unwrap();
        assert_eq!(sk.name, "mc-abcd1234-rcon");
        assert_eq!(sk.key, "password");
    }

    #[test]
    fn statefulset_pvc_template_uses_storage_class() {
        let sts = build_statefulset(&params());
        let vct = &sts
            .spec
            .as_ref()
            .unwrap()
            .volume_claim_templates
            .as_ref()
            .unwrap()[0];
        let pvc_spec = vct.spec.as_ref().unwrap();
        assert_eq!(pvc_spec.storage_class_name.as_deref(), Some("tank"));
        let storage = pvc_spec
            .resources
            .as_ref()
            .unwrap()
            .requests
            .as_ref()
            .unwrap()
            .get("storage")
            .unwrap();
        assert_eq!(storage.0, "10Gi");
        assert_eq!(
            pvc_spec.access_modes.as_ref().unwrap(),
            &vec!["ReadWriteOnce".to_owned()]
        );
    }

    #[test]
    fn statefulset_pvc_template_omits_storage_class_when_none() {
        let mut p = params();
        p.storage_class = None;
        let sts = build_statefulset(&p);
        let vct = &sts
            .spec
            .as_ref()
            .unwrap()
            .volume_claim_templates
            .as_ref()
            .unwrap()[0];
        assert!(vct.spec.as_ref().unwrap().storage_class_name.is_none());
    }

    #[test]
    fn statefulset_service_name_points_at_headless() {
        let sts = build_statefulset(&params());
        assert_eq!(
            sts.spec.as_ref().unwrap().service_name.as_deref(),
            Some("mc-abcd1234-headless")
        );
    }

    #[test]
    fn statefulset_container_exposes_mc_and_rcon_ports() {
        let sts = build_statefulset(&params());
        let ports = sts
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers[0]
            .ports
            .as_ref()
            .unwrap();
        let by_name: BTreeMap<&str, i32> = ports
            .iter()
            .filter_map(|p| p.name.as_deref().map(|n| (n, p.container_port)))
            .collect();
        assert_eq!(by_name.get("mc"), Some(&i32::from(MC_PORT)));
        assert_eq!(by_name.get("rcon"), Some(&i32::from(RCON_PORT)));
    }

    #[test]
    fn headless_service_name_uses_headless_suffix() {
        let svc = build_headless_service(&params());
        assert_eq!(svc.metadata.name.as_deref(), Some("mc-abcd1234-headless"));
        assert_eq!(svc.metadata.namespace.as_deref(), Some("mc"));
    }

    #[test]
    fn headless_service_is_clusterip_none_with_publish_not_ready() {
        let svc = build_headless_service(&params());
        let spec = svc.spec.as_ref().unwrap();
        assert_eq!(spec.cluster_ip.as_deref(), Some("None"));
        assert_eq!(spec.publish_not_ready_addresses, Some(true));
    }

    #[test]
    fn headless_service_exposes_only_rcon_port() {
        let svc = build_headless_service(&params());
        let ports = svc.spec.as_ref().unwrap().ports.as_ref().unwrap();
        assert_eq!(ports.len(), 1);
        let port = &ports[0];
        assert_eq!(port.port, i32::from(RCON_PORT));
        assert_eq!(
            port.target_port,
            Some(IntOrString::Int(i32::from(RCON_PORT)))
        );
        assert_eq!(port.name.as_deref(), Some("rcon"));
        assert_eq!(port.protocol.as_deref(), Some("TCP"));
    }

    #[test]
    fn headless_service_selects_managed_pod() {
        let svc = build_headless_service(&params());
        let selector = svc.spec.as_ref().unwrap().selector.as_ref().unwrap();
        assert_eq!(
            selector.get("app.anvil.io/server").map(String::as_str),
            Some("abcd1234")
        );
        assert_eq!(
            selector.get("app.anvil.io/managed-by").map(String::as_str),
            Some("anvil")
        );
    }

    #[test]
    fn service_loadbalancer_has_no_nodeport() {
        let svc = build_service(&params());
        let spec = svc.spec.as_ref().unwrap();
        assert_eq!(spec.type_.as_deref(), Some("LoadBalancer"));
        let port = &spec.ports.as_ref().unwrap()[0];
        assert_eq!(port.port, i32::from(MC_PORT));
        assert_eq!(port.node_port, None);
    }

    #[test]
    fn public_service_never_exposes_rcon_port() {
        for mode in ["loadbalancer", "nodeport", "clusterip"] {
            let mut p = params();
            p.exposure_mode = mode;
            if mode == "nodeport" {
                p.nodeport = Some(30_005);
            }
            let svc = build_service(&p);
            let ports = svc.spec.as_ref().unwrap().ports.as_ref().unwrap();
            assert_eq!(
                ports.len(),
                1,
                "{mode}: public Service must have exactly one port"
            );
            assert_ne!(
                ports[0].port,
                i32::from(RCON_PORT),
                "{mode}: RCON must never appear on the public Service"
            );
        }
    }

    #[test]
    fn service_nodeport_uses_assigned_port() {
        let mut p = params();
        p.exposure_mode = "nodeport";
        p.nodeport = Some(30_005);
        let svc = build_service(&p);
        let spec = svc.spec.as_ref().unwrap();
        assert_eq!(spec.type_.as_deref(), Some("NodePort"));
        let port = &spec.ports.as_ref().unwrap()[0];
        assert_eq!(port.node_port, Some(30_005));
    }

    #[test]
    fn service_clusterip_type_set() {
        let mut p = params();
        p.exposure_mode = "clusterip";
        let svc = build_service(&p);
        assert_eq!(
            svc.spec.as_ref().unwrap().type_.as_deref(),
            Some("ClusterIP")
        );
    }

    #[test]
    fn rcon_secret_name_namespace_and_data() {
        let sec = build_rcon_secret("abcd1234", "mc", "passw0rd");
        assert_eq!(sec.metadata.name.as_deref(), Some("mc-abcd1234-rcon"));
        assert_eq!(sec.metadata.namespace.as_deref(), Some("mc"));
        assert_eq!(sec.type_.as_deref(), Some("Opaque"));
        let data = sec.data.as_ref().unwrap();
        assert_eq!(data["password"].0, b"passw0rd".to_vec());
    }

    #[test]
    fn rcon_password_is_24_alphanumeric() {
        let p = rcon_password();
        assert_eq!(p.len(), 24);
        assert!(p.chars().all(|c| c.is_ascii_alphanumeric()));
    }
}
