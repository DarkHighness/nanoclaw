use crate::{Result, ToolError};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub use sandbox::{
    GrantedFilesystemPermissions, GrantedNetworkPermissions, GrantedPermissionProfile,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PermissionGrantScope {
    #[default]
    Turn,
    Session,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileSystemPermissionRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write: Option<Vec<String>>,
}

impl FileSystemPermissionRequest {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.read.as_ref().is_none_or(Vec::is_empty)
            && self.write.as_ref().is_none_or(Vec::is_empty)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NetworkPermissionRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_domains: Option<Vec<String>>,
}

impl NetworkPermissionRequest {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.enabled.is_none() && self.allow_domains.as_ref().is_none_or(Vec::is_empty)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RequestPermissionProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkPermissionRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_system: Option<FileSystemPermissionRequest>,
}

impl RequestPermissionProfile {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.network
            .as_ref()
            .is_none_or(NetworkPermissionRequest::is_empty)
            && self
                .file_system
                .as_ref()
                .is_none_or(FileSystemPermissionRequest::is_empty)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RequestPermissionsArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub permissions: RequestPermissionProfile,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RequestPermissionsResponse {
    pub permissions: RequestPermissionProfile,
    #[serde(default)]
    pub scope: PermissionGrantScope,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionRequest {
    pub reason: Option<String>,
    pub permissions: GrantedPermissionProfile,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrantedPermissionResponse {
    pub permissions: GrantedPermissionProfile,
    pub scope: PermissionGrantScope,
}

#[async_trait]
pub trait PermissionRequestHandler: Send + Sync {
    async fn request_permissions(
        &self,
        request: PermissionRequest,
    ) -> Result<GrantedPermissionResponse>;
}

pub fn normalize_request_permission_profile(
    profile: &RequestPermissionProfile,
    base_root: &Path,
) -> Result<GrantedPermissionProfile> {
    let file_system = profile
        .file_system
        .as_ref()
        .map(|request| normalize_file_system_permissions(request, base_root))
        .transpose()?
        .unwrap_or_default();
    let network = profile
        .network
        .as_ref()
        .map(normalize_network_permissions)
        .transpose()?
        .flatten();

    Ok(GrantedPermissionProfile {
        file_system,
        network,
    })
}

pub fn request_permission_profile_from_granted(
    profile: &GrantedPermissionProfile,
) -> RequestPermissionProfile {
    RequestPermissionProfile {
        network: profile.network.as_ref().map(|network| match network {
            GrantedNetworkPermissions::Full => NetworkPermissionRequest {
                enabled: Some(true),
                allow_domains: None,
            },
            GrantedNetworkPermissions::AllowDomains(domains) => NetworkPermissionRequest {
                enabled: None,
                allow_domains: Some(domains.clone()),
            },
        }),
        file_system: (!profile.file_system.is_empty()).then(|| FileSystemPermissionRequest {
            read: (!profile.file_system.read_roots.is_empty()).then(|| {
                profile
                    .file_system
                    .read_roots
                    .iter()
                    .map(|path| path_to_string(path))
                    .collect()
            }),
            write: (!profile.file_system.write_roots.is_empty()).then(|| {
                profile
                    .file_system
                    .write_roots
                    .iter()
                    .map(|path| path_to_string(path))
                    .collect()
            }),
        }),
    }
}

#[must_use]
pub fn granted_permissions_are_subset(
    requested: &GrantedPermissionProfile,
    granted: &GrantedPermissionProfile,
) -> bool {
    path_roots_are_subset(
        &requested.file_system.read_roots,
        &granted.file_system.read_roots,
    ) && path_roots_are_subset(
        &requested.file_system.write_roots,
        &granted.file_system.write_roots,
    ) && network_permissions_are_subset(requested.network.as_ref(), granted.network.as_ref())
}

fn normalize_file_system_permissions(
    request: &FileSystemPermissionRequest,
    base_root: &Path,
) -> Result<GrantedFilesystemPermissions> {
    Ok(GrantedFilesystemPermissions {
        read_roots: normalize_permission_roots(request.read.as_deref(), base_root)?,
        write_roots: normalize_permission_roots(request.write.as_deref(), base_root)?,
    })
}

fn normalize_network_permissions(
    request: &NetworkPermissionRequest,
) -> Result<Option<GrantedNetworkPermissions>> {
    if request.enabled == Some(false) {
        return Err(ToolError::invalid(
            "request_permissions network.enabled must be true when requesting additional access",
        ));
    }
    match (request.enabled, request.allow_domains.as_ref()) {
        (Some(true), Some(_)) => Err(ToolError::invalid(
            "request_permissions cannot ask for network.enabled=true and network.allow_domains together",
        )),
        (Some(true), None) => Ok(Some(GrantedNetworkPermissions::Full)),
        (None, Some(domains)) => {
            let domains = domains
                .iter()
                .map(|domain| domain.trim())
                .filter(|domain| !domain.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            if domains.is_empty() {
                return Err(ToolError::invalid(
                    "request_permissions network.allow_domains cannot be empty",
                ));
            }
            Ok(Some(GrantedNetworkPermissions::AllowDomains(domains)))
        }
        (None, None) => Ok(None),
        (Some(false), _) => unreachable!("validated above"),
    }
}

fn normalize_permission_roots(roots: Option<&[String]>, base_root: &Path) -> Result<Vec<PathBuf>> {
    let Some(roots) = roots else {
        return Ok(Vec::new());
    };

    let mut normalized = Vec::with_capacity(roots.len());
    for root in roots {
        let trimmed = root.trim();
        if trimmed.is_empty() {
            return Err(ToolError::invalid(
                "request_permissions filesystem entries cannot be empty",
            ));
        }
        let candidate = Path::new(trimmed);
        let resolved = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            base_root.join(candidate)
        };
        normalized.push(sandbox::normalize_granted_permission_path(&resolved)?);
    }
    normalized.sort();
    normalized.dedup();
    Ok(normalized)
}

fn path_roots_are_subset(requested: &[PathBuf], granted: &[PathBuf]) -> bool {
    granted.iter().all(|granted_root| {
        requested
            .iter()
            .any(|requested_root| granted_root.starts_with(requested_root))
    })
}

fn network_permissions_are_subset(
    requested: Option<&GrantedNetworkPermissions>,
    granted: Option<&GrantedNetworkPermissions>,
) -> bool {
    match (requested, granted) {
        (_, None) => true,
        (Some(GrantedNetworkPermissions::Full), Some(_)) => true,
        (
            Some(GrantedNetworkPermissions::AllowDomains(requested_domains)),
            Some(GrantedNetworkPermissions::AllowDomains(granted_domains)),
        ) => granted_domains.iter().all(|domain| {
            requested_domains
                .iter()
                .any(|requested| requested == domain)
        }),
        _ => false,
    }
}

fn path_to_string(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        FileSystemPermissionRequest, GrantedNetworkPermissions, NetworkPermissionRequest,
        RequestPermissionProfile, granted_permissions_are_subset,
        normalize_request_permission_profile, request_permission_profile_from_granted,
    };
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    #[test]
    fn normalize_request_permission_profile_resolves_relative_paths() {
        let workspace = tempdir().unwrap();
        let profile = RequestPermissionProfile {
            file_system: Some(FileSystemPermissionRequest {
                read: Some(vec!["src".to_string()]),
                write: Some(vec!["tmp/output".to_string()]),
            }),
            network: None,
        };

        let normalized = normalize_request_permission_profile(&profile, workspace.path()).unwrap();
        assert_eq!(
            normalized.file_system.read_roots,
            vec![workspace.path().join("src")]
        );
        assert_eq!(
            normalized.file_system.write_roots,
            vec![workspace.path().join("tmp/output")]
        );
    }

    #[test]
    fn request_permission_profile_round_trips_granted_permissions() {
        let profile = super::GrantedPermissionProfile {
            file_system: super::GrantedFilesystemPermissions {
                read_roots: vec![PathBuf::from("/tmp/read")],
                write_roots: vec![PathBuf::from("/tmp/write")],
            },
            network: Some(GrantedNetworkPermissions::AllowDomains(vec![
                "example.com".to_string(),
            ])),
        };

        let round_tripped = request_permission_profile_from_granted(&profile);
        assert_eq!(
            round_tripped.file_system.unwrap().write.unwrap(),
            vec!["/tmp/write".to_string()]
        );
        assert_eq!(
            round_tripped.network.unwrap().allow_domains.unwrap(),
            vec!["example.com".to_string()]
        );
    }

    #[test]
    fn granted_permissions_must_stay_within_requested_subset() {
        let requested = normalize_request_permission_profile(
            &RequestPermissionProfile {
                file_system: Some(FileSystemPermissionRequest {
                    read: Some(vec!["/tmp/workspace".to_string()]),
                    write: Some(vec!["/tmp/workspace".to_string()]),
                }),
                network: Some(NetworkPermissionRequest {
                    enabled: None,
                    allow_domains: Some(vec!["example.com".to_string()]),
                }),
            },
            Path::new("/"),
        )
        .unwrap();
        let granted = normalize_request_permission_profile(
            &RequestPermissionProfile {
                file_system: Some(FileSystemPermissionRequest {
                    read: Some(vec!["/tmp/workspace/subdir".to_string()]),
                    write: None,
                }),
                network: Some(NetworkPermissionRequest {
                    enabled: None,
                    allow_domains: Some(vec!["example.com".to_string()]),
                }),
            },
            Path::new("/"),
        )
        .unwrap();

        assert!(granted_permissions_are_subset(&requested, &granted));
    }
}
