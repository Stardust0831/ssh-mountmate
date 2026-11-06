//! Read-only directory discovery. Native SFTP permissions remain authoritative.
use crate::{Connection, Error, Session, display_value, transport};
use mountmate_core::connection::ConnectionDraft;
use mountmate_core::model::MountMapping;
use serde::Deserialize;
use std::collections::HashSet;

pub(crate) const SCHEMA: &str = "ssh.mount-roots/v1";
const PAGE_SIZE: usize = 20;
// Twenty maximum-length Unicode names and paths, including JSON surrogate escapes.
const PAGE_BODY_LIMIT: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MountRootKind {
    Home,
    OrgShare,
    OrgUserDir,
    ProjectRoot,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MountRole {
    Read,
    Write,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct MountRoot {
    pub id: String,
    pub display_name: String,
    pub kind: MountRootKind,
    pub path: String,
    pub role: MountRole,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MountRootPage {
    schema_version: String,
    site_id: String,
    pub items: Vec<MountRoot>,
    pub next_cursor: Option<String>,
}

impl Session {
    pub fn has_mount_directory_discovery(&self) -> bool {
        self.client.mount_roots
    }

    pub async fn mount_roots(
        &self,
        site: &Connection,
        cursor: Option<&str>,
    ) -> Result<MountRootPage, Error> {
        self.check_expiry()?;
        if !self.client.mount_roots {
            return Err(Error::Unavailable);
        }
        if !self.mount_roots_granted {
            return Err(Error::Forbidden);
        }
        if !self.connections().contains(site) || cursor.is_some_and(|value| !valid_cursor(value)) {
            return Err(Error::Contract);
        }
        let mut url = self
            .client
            .portal
            .join("/v1/account/ssh/mount-roots")
            .map_err(|_| Error::Contract)?;
        url.query_pairs_mut()
            .append_pair("site_id", &site.site_id)
            .append_pair("limit", &PAGE_SIZE.to_string());
        if let Some(cursor) = cursor {
            url.query_pairs_mut().append_pair("cursor", cursor);
        }
        let response = self
            .client
            .http
            .send_bounded(
                self.client
                    .http
                    .client
                    .get(url)
                    .bearer_auth(self.token.as_str()),
                PAGE_BODY_LIMIT,
            )
            .await?;
        if matches!(response.status().as_u16(), 404 | 501 | 503) {
            return Err(Error::Unavailable);
        }
        if response.status().as_u16() == 400 {
            let body: serde_json::Value =
                serde_json::from_slice(response.body()).map_err(|_| Error::Contract)?;
            return Err(if body["detail"]["code"] == "invalid_mount_roots_cursor" {
                Error::DirectoryChanged
            } else {
                Error::Contract
            });
        }
        let page: MountRootPage = transport::json(response)?;
        page.validate(&site.site_id, cursor)?;
        Ok(page)
    }
}

impl MountRootPage {
    fn validate(&self, site: &str, cursor: Option<&str>) -> Result<(), Error> {
        let mut ids = HashSet::new();
        if self.schema_version != SCHEMA
            || self.site_id != site
            || self.items.len() > PAGE_SIZE
            || self
                .next_cursor
                .as_deref()
                .is_some_and(|next| !valid_cursor(next) || Some(next) == cursor)
            || self.items.iter().any(|item| {
                !display_value(&item.id, 255)
                    || !ids.insert(&item.id)
                    || !directory_text(&item.display_name, 2048)
                    || !directory_text(&item.path, 4096)
                    || !item.path.starts_with('/')
                    || item.path == "/"
                    || item.path.split('/').any(|part| matches!(part, "." | ".."))
            })
        {
            return Err(Error::Contract);
        }
        Ok(())
    }
}

fn valid_cursor(value: &str) -> bool {
    display_value(value, 101)
}

fn directory_text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= limit
        && !value.chars().any(char::is_control)
}

pub fn apply_selected_roots(draft: &mut ConnectionDraft, roots: &[MountRoot]) {
    let mut unused_primary = draft.editing_id.is_none()
        && draft.primary_mount
        && draft.remote_path.is_empty()
        && draft.mountpoint.is_empty();
    for root in roots {
        if (draft.primary_mount && draft.remote_path == root.path)
            || draft
                .mounts
                .iter()
                .any(|mapping| mapping.remote_path == root.path)
        {
            continue;
        }
        if unused_primary {
            draft.remote_path.clone_from(&root.path);
            unused_primary = false;
        } else {
            draft.mounts.push(MountMapping {
                remote_path: root.path.clone(),
                ..MountMapping::default()
            });
        }
    }
}
