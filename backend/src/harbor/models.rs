use serde::{Deserialize, Serialize};

/// Request to create a user in Harbor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateHarborUserRequest {
    pub username: String,
    pub password: String,
    pub realname: String,
    pub email: Option<String>,
    pub comment: Option<String>,
}

/// Harbor user info from GET /api/v2.0/users
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarborUser {
    #[serde(alias = "user_id")]
    pub user_id: i64,
    pub username: String,
    pub email: Option<String>,
    pub realname: Option<String>,
    #[serde(alias = "sysadmin_flag")]
    pub sysadmin_flag: Option<bool>,
    #[serde(alias = "admin_role_in_auth")]
    pub admin_role_in_auth: Option<bool>,
}

/// Request to change a Harbor user's password
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangePasswordRequest {
    pub old_password: String,
    pub new_password: String,
}

/// Harbor project metadata (key-value map)
pub type ProjectMetadata = std::collections::HashMap<String, String>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarborProject {
    #[serde(alias = "project_id")]
    pub project_id: i64,
    #[serde(alias = "owner_id")]
    pub owner_id: i64,
    pub name: String,
    #[serde(alias = "owner_name")]
    pub owner_name: Option<String>,
    #[serde(alias = "repo_count")]
    pub repo_count: Option<i64>,
    #[serde(alias = "current_user_role_id")]
    pub current_user_role_id: Option<i32>,
    #[serde(alias = "current_user_role_ids")]
    pub current_user_role_ids: Option<Vec<i32>>,
    #[serde(alias = "creation_time")]
    pub creation_time: Option<String>,
    #[serde(alias = "update_time")]
    pub update_time: Option<String>,
    pub metadata: Option<ProjectMetadata>,
    pub registry_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSummary {
    #[serde(alias = "repo_count")]
    pub repo_count: i64,
    #[serde(alias = "project_admin_count")]
    pub project_admin_count: Option<i64>,
    #[serde(alias = "maintainer_count")]
    pub maintainer_count: Option<i64>,
    #[serde(alias = "developer_count")]
    pub developer_count: Option<i64>,
    #[serde(alias = "guest_count")]
    pub guest_count: Option<i64>,
    #[serde(alias = "limited_guest_count")]
    pub limited_guest_count: Option<i64>,
    pub quota: Option<ProjectSummaryQuota>,
    pub registry: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSummaryQuota {
    pub hard: Option<serde_json::Value>,
    pub used: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarborRepository {
    pub id: i64,
    #[serde(alias = "project_id")]
    pub project_id: i64,
    pub name: String,
    pub description: Option<String>,
    #[serde(alias = "artifact_count")]
    pub artifact_count: Option<i64>,
    #[serde(alias = "pull_count")]
    pub pull_count: Option<i64>,
    #[serde(alias = "creation_time")]
    pub creation_time: Option<String>,
    #[serde(alias = "update_time")]
    pub update_time: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarborArtifact {
    pub id: i64,
    #[serde(alias = "digest")]
    pub digest: Option<String>,
    #[serde(alias = "size")]
    pub size: Option<i64>,
    #[serde(alias = "push_time")]
    pub push_time: Option<String>,
    #[serde(alias = "pull_time")]
    pub pull_time: Option<String>,
    pub tags: Option<Vec<ArtifactTag>>,
    #[serde(alias = "extra_attrs")]
    pub extra_attrs: Option<serde_json::Value>,
    #[serde(alias = "manifest_media_type")]
    pub manifest_media_type: Option<String>,
    #[serde(alias = "media_type")]
    pub media_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactTag {
    pub name: String,
    #[serde(alias = "push_time")]
    pub push_time: Option<String>,
    #[serde(alias = "pull_time")]
    pub pull_time: Option<String>,
    #[serde(alias = "signed")]
    pub signed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarborMember {
    pub id: i64,
    #[serde(alias = "project_id")]
    pub project_id: i64,
    #[serde(alias = "entity_name")]
    pub entity_name: String,
    #[serde(alias = "role_name")]
    pub role_name: String,
    #[serde(alias = "role_id")]
    pub role_id: i32,
    #[serde(alias = "entity_id")]
    pub entity_id: i64,
    #[serde(alias = "entity_type")]
    pub entity_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateProjectRequest {
    #[serde(alias = "project_name")]
    pub project_name: String,
    pub metadata: ProjectMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMemberRequest {
    #[serde(alias = "role_id")]
    pub role_id: i32,
    #[serde(alias = "member_user")]
    pub member_user: Option<MemberUser>,
    #[serde(alias = "member_group")]
    pub member_group: Option<MemberGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberUser {
    #[serde(alias = "user_id")]
    pub user_id: Option<i64>,
    pub username: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberGroup {
    pub id: Option<i64>,
    #[serde(alias = "group_name")]
    pub group_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarborStatistics {
    pub total_projects: i64,
    pub total_repositories: i64,
    pub total_artifacts: i64,
    pub total_pull_count: i64,
    pub public_project_count: i64,
    pub private_project_count: i64,
    pub top_repositories: Vec<RepoStat>,
    pub recent_projects: Vec<HarborProject>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoStat {
    pub name: String,
    pub project_name: String,
    pub pull_count: i64,
    pub artifact_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarborInfo {
    pub registry_url: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectQuery {
    pub name: Option<String>,
    pub public: Option<bool>,
    pub page: Option<i64>,
    #[serde(alias = "page_size")]
    pub page_size: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarborRegistry {
    pub id: i64,
    pub name: String,
    pub url: String,
    #[serde(alias = "type")]
    pub registry_type: String,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryCredential {
    #[serde(rename = "type")]
    pub credential_type: String,
    #[serde(rename = "access_key")]
    pub access_key: String,
    #[serde(rename = "access_secret")]
    pub access_secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRegistryRequest {
    pub name: String,
    pub url: String,
    #[serde(rename = "type")]
    pub registry_type: String,
    pub credential: RegistryCredential,
    pub insecure: bool,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationTrigger {
    #[serde(alias = "trigger_settings")]
    pub trigger_settings: Option<serde_json::Value>,
    #[serde(rename = "type")]
    pub trigger_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationFilter {
    #[serde(rename = "type")]
    pub filter_type: String,
    pub value: String,
}

/// Minimal registry reference used when creating a replication policy.
/// Harbor expects `src_registry`/`dest_registry` to be objects containing an `id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryEntity {
    pub id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateReplicationPolicyRequest {
    pub name: String,
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub src_registry: Option<RegistryEntity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dest_registry: Option<RegistryEntity>,
    pub dest_namespace: String,
    pub trigger: ReplicationTrigger,
    pub filters: Vec<ReplicationFilter>,
    pub enabled: bool,
    pub deletion: bool,
    #[serde(rename = "override")]
    pub override_: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationPolicy {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationExecutionRequest {
    #[serde(alias = "policy_id")]
    pub policy_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationExecution {
    pub id: i64,
    #[serde(alias = "policy_id")]
    pub policy_id: i64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarborWebhookPayload {
    #[serde(alias = "type")]
    pub event_type: String,
    #[serde(alias = "occur_at")]
    pub occur_at: Option<serde_json::Value>,
    pub operator: Option<String>,
    #[serde(alias = "event_data")]
    pub event_data: HarborWebhookEventData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarborWebhookEventData {
    pub resources: Vec<HarborWebhookResource>,
    pub repository: HarborWebhookRepository,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarborWebhookResource {
    pub digest: Option<String>,
    pub tag: Option<String>,
    #[serde(alias = "resource_url")]
    pub resource_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarborWebhookRepository {
    pub name: String,
    pub namespace: String,
    #[serde(alias = "repo_full_name")]
    pub repo_full_name: Option<String>,
}
