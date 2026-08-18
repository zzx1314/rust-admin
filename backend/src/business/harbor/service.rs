use crate::common::error::AppError;
use crate::common::pagination::PageResponse;
use crate::common::util::format_iso_datetime;
use crate::business::harbor::client::HarborClient;
use crate::business::harbor::models::{
    ChangePasswordRequest, CreateHarborUserRequest, CreateMemberRequest, CreateProjectRequest,
    CreateRegistryRequest, CreateReplicationPolicyRequest, HarborArtifact, HarborInfo, HarborMember,
    HarborProject, HarborRegistry, HarborRepository, HarborStatistics, HarborUser, ProjectQuery, ProjectSummary,
    RegistryCredential, RegistryEntity, ReplicationExecution, ReplicationExecutionRequest, ReplicationFilter,
    ReplicationPolicy, ReplicationTrigger, RepoStat,
};
use std::sync::Arc;

pub struct HarborService {
    client: Arc<HarborClient>,
    registry_endpoint_id: Option<i64>,
    registry_insecure: Option<bool>,
    replication_timeout_secs: u64,
    replication_poll_interval_secs: u64,
}

impl HarborService {
    pub fn new(client: Arc<HarborClient>) -> Self {
        Self {
            client,
            registry_endpoint_id: None,
            registry_insecure: None,
            replication_timeout_secs: 30,
            replication_poll_interval_secs: 1,
        }
    }

    pub fn with_registry_endpoint_id(mut self, id: Option<i64>) -> Self {
        self.registry_endpoint_id = id;
        self
    }

    pub fn with_registry_insecure(mut self, insecure: Option<bool>) -> Self {
        self.registry_insecure = insecure;
        self
    }

    pub fn with_replication_timeout_secs(mut self, secs: u64) -> Self {
        self.replication_timeout_secs = secs;
        self
    }

    pub fn with_replication_poll_interval_secs(mut self, secs: u64) -> Self {
        self.replication_poll_interval_secs = secs.max(1);
        self
    }

    pub fn get_info(&self) -> HarborInfo {
        let registry_url = if self.client.base_url.is_empty() {
            String::new()
        } else {
            // Extract host:port from the harbor URL for docker commands
            // e.g., "http://localhost:8097" -> "localhost:8097"
            self.client.base_url
                .trim_start_matches("http://")
                .trim_start_matches("https://")
                .trim_end_matches('/')
                .to_string()
        };
        HarborInfo {
            registry_url,
            enabled: self.client.enabled,
        }
    }

    fn ensure_enabled(&self) -> Result<(), AppError> {
        if !self.client.enabled {
            return Err(AppError::BadRequest(
                "Harbor is not configured, please check backend/config.toml".to_string(),
            ));
        }
        Ok(())
    }

    fn map_harbor_error(status: reqwest::StatusCode, body: &str) -> AppError {
        match status.as_u16() {
            401 => AppError::Unauthorized(format!("Harbor unauthorized: {}", body)),
            403 => AppError::AuthError(format!("Harbor forbidden: {}", body)),
            404 => AppError::NotFound(format!("Harbor not found: {}", body)),
            _ => AppError::BadRequest(format!("Harbor error ({}): {}", status, body)),
        }
    }

    async fn check_response(&self, response: reqwest::Response) -> Result<(), AppError> {
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        Err(Self::map_harbor_error(status, &body))
    }

    fn extract_total_count(response: &reqwest::Response) -> i64 {
        response
            .headers()
            .get("x-total-count")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0)
    }

    async fn build_paginated_response<T>(
        &self,
        response: reqwest::Response,
        page: i64,
        page_size: i64,
    ) -> Result<PageResponse<T>, AppError>
    where
        T: serde::de::DeserializeOwned,
    {
        let total = Self::extract_total_count(&response);

        let records: Vec<T> = response.json().await.map_err(|e| {
            AppError::BadRequest(format!("Failed to parse Harbor response: {}", e))
        })?;

        Ok(PageResponse::new(records, total, page, page_size))
    }

    pub async fn list_projects(
        &self,
        query: &ProjectQuery,
    ) -> Result<PageResponse<HarborProject>, AppError> {
        self.ensure_enabled()?;
        let mut url = reqwest::Url::parse(&self.client.api_url("/api/v2.0/projects"))
            .map_err(|e| AppError::BadRequest(format!("Invalid Harbor URL: {}", e)))?;

        if let Some(name) = &query.name {
            url.query_pairs_mut().append_pair("name", name);
        }
        if let Some(public) = query.public {
            url.query_pairs_mut()
                .append_pair("public", &public.to_string());
        }
        let page = query.page.unwrap_or(1);
        let page_size = query.page_size.unwrap_or(10);
        url.query_pairs_mut().append_pair("page", &page.to_string());
        url.query_pairs_mut()
            .append_pair("page_size", &page_size.to_string());

        let response = self
            .client
            .client
            .get(url)
            .headers(self.client.default_headers())
            .send()
            .await
            .map_err(|e| AppError::BadRequest(format!("Harbor request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(Self::map_harbor_error(status, &body));
        }

        let mut result = self.build_paginated_response::<HarborProject>(response, page, page_size).await?;
        result.records.iter_mut().for_each(|p| {
            p.creation_time = p.creation_time.as_deref().map(format_iso_datetime);
            p.update_time = p.update_time.as_deref().map(format_iso_datetime);
        });
        Ok(result)
    }

    pub async fn get_project_summary(
        &self,
        project_name: &str,
    ) -> Result<ProjectSummary, AppError> {
        self.ensure_enabled()?;
        let path = format!("/api/v2.0/projects/{}/summary", project_name);
        let url = reqwest::Url::parse(&self.client.api_url(&path))
            .map_err(|e| AppError::BadRequest(format!("Invalid Harbor URL: {}", e)))?;

        let response = self
            .client
            .client
            .get(url)
            .headers(self.client.default_headers())
            .send()
            .await
            .map_err(|e| AppError::BadRequest(format!("Harbor request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(Self::map_harbor_error(status, &body));
        }

        response.json::<ProjectSummary>().await.map_err(|e| {
            AppError::BadRequest(format!("Failed to parse Harbor response: {}", e))
        })
    }

    pub async fn list_repositories(
        &self,
        project_name: &str,
        page: Option<i64>,
        page_size: Option<i64>,
    ) -> Result<PageResponse<HarborRepository>, AppError> {
        self.ensure_enabled()?;
        let path = format!("/api/v2.0/projects/{}/repositories", project_name);
        let mut url = reqwest::Url::parse(&self.client.api_url(&path))
            .map_err(|e| AppError::BadRequest(format!("Invalid Harbor URL: {}", e)))?;

        let page = page.unwrap_or(1);
        let page_size = page_size.unwrap_or(10);
        url.query_pairs_mut().append_pair("page", &page.to_string());
        url.query_pairs_mut()
            .append_pair("page_size", &page_size.to_string());

        let response = self
            .client
            .client
            .get(url)
            .headers(self.client.default_headers())
            .send()
            .await
            .map_err(|e| AppError::BadRequest(format!("Harbor request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(Self::map_harbor_error(status, &body));
        }

        let mut result = self.build_paginated_response::<HarborRepository>(response, page, page_size).await?;
        result.records.iter_mut().for_each(|r| {
            r.creation_time = r.creation_time.as_deref().map(format_iso_datetime);
            r.update_time = r.update_time.as_deref().map(format_iso_datetime);
        });
        Ok(result)
    }

    pub async fn list_members(
        &self,
        project_name: &str,
        page: Option<i64>,
        page_size: Option<i64>,
    ) -> Result<PageResponse<HarborMember>, AppError> {
        self.ensure_enabled()?;
        let path = format!("/api/v2.0/projects/{}/members", project_name);
        let mut url = reqwest::Url::parse(&self.client.api_url(&path))
            .map_err(|e| AppError::BadRequest(format!("Invalid Harbor URL: {}", e)))?;

        let page = page.unwrap_or(1);
        let page_size = page_size.unwrap_or(10);
        url.query_pairs_mut().append_pair("page", &page.to_string());
        url.query_pairs_mut()
            .append_pair("page_size", &page_size.to_string());

        let response = self
            .client
            .client
            .get(url)
            .headers(self.client.default_headers())
            .send()
            .await
            .map_err(|e| AppError::BadRequest(format!("Harbor request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(Self::map_harbor_error(status, &body));
        }

        self.build_paginated_response::<HarborMember>(response, page, page_size).await
    }

    pub async fn list_artifacts(
        &self,
        project_name: &str,
        repo_name: &str,
        page: Option<i64>,
        page_size: Option<i64>,
    ) -> Result<PageResponse<HarborArtifact>, AppError> {
        self.ensure_enabled()?;
        // Harbor API: /api/v2.0/projects/{project}/repositories/{repo_name}/artifacts
        // Harbor repo names come as "{project}/{short_name}" (e.g. "appstore/redis"),
        // but the artifacts endpoint expects just the short name.
        let short_name = repo_name.split('/').next_back().unwrap_or(repo_name);
        let path = format!(
            "/api/v2.0/projects/{}/repositories/{}/artifacts",
            project_name,
            short_name,
        );
        let mut url = reqwest::Url::parse(&self.client.api_url(&path))
            .map_err(|e| AppError::BadRequest(format!("Invalid Harbor URL: {}", e)))?;

        url.query_pairs_mut()
            .append_pair("with_tag", "true");
        let page = page.unwrap_or(1);
        let page_size = page_size.unwrap_or(10);
        url.query_pairs_mut().append_pair("page", &page.to_string());
        url.query_pairs_mut()
            .append_pair("page_size", &page_size.to_string());

        let response = self
            .client
            .client
            .get(url)
            .headers(self.client.default_headers())
            .send()
            .await
            .map_err(|e| AppError::BadRequest(format!("Harbor request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(Self::map_harbor_error(status, &body));
        }

        let mut result = self.build_paginated_response::<HarborArtifact>(response, page, page_size).await?;
        result.records.iter_mut().for_each(|a| {
            a.push_time = a.push_time.as_deref().map(format_iso_datetime);
            a.pull_time = a.pull_time.as_deref().map(format_iso_datetime);
        });
        Ok(result)
    }

    pub async fn get_statistics(&self) -> Result<HarborStatistics, AppError> {
        self.ensure_enabled()?;

        // 1. Fetch all projects
        let query = ProjectQuery {
            name: None,
            public: None,
            page: Some(1),
            page_size: Some(100),
        };
        let projects_page = self.list_projects(&query).await?;

        let total_projects = projects_page.total;
        let mut public_count = 0i64;
        let mut private_count = 0i64;
        for p in &projects_page.records {
            let is_pub = p.metadata.as_ref()
                .and_then(|m| m.get("public"))
                .map(|v| v == "true")
                .unwrap_or(false);
            if is_pub { public_count += 1; } else { private_count += 1; }
        }

        // 2. Fetch repositories for all projects
        let mut total_repos = 0i64;
        let mut total_artifacts = 0i64;
        let mut total_pulls = 0i64;
        let mut all_repos: Vec<RepoStat> = Vec::new();

        for project in &projects_page.records {
            let repo_page = self.list_repositories(
                &project.name,
                Some(1),
                Some(100),
            ).await?;

            total_repos += repo_page.total;

            for repo in &repo_page.records {
                let pull = repo.pull_count.unwrap_or(0);
                let artifact = repo.artifact_count.unwrap_or(0);
                total_pulls += pull;
                total_artifacts += artifact;
                all_repos.push(RepoStat {
                    name: repo.name.clone(),
                    project_name: project.name.clone(),
                    pull_count: pull,
                    artifact_count: artifact,
                });
            }
        }

        // 3. Sort by pull_count descending, take top 5
        all_repos.sort_by_key(|repo| std::cmp::Reverse(repo.pull_count));
        all_repos.truncate(5);

        // 4. Get recent 5 projects (sorted by creation_time descending)
        let mut recent = projects_page.records.clone();
        recent.sort_by(|a, b| {
            b.creation_time.as_deref().unwrap_or("")
                .cmp(a.creation_time.as_deref().unwrap_or(""))
        });
        recent.truncate(5);

        Ok(HarborStatistics {
            total_projects,
            total_repositories: total_repos,
            total_artifacts,
            total_pull_count: total_pulls,
            public_project_count: public_count,
            private_project_count: private_count,
            top_repositories: all_repos,
            recent_projects: recent,
        })
    }

    pub async fn create_user(&self, req: &CreateHarborUserRequest) -> Result<(), AppError> {
        self.ensure_enabled()?;
        let url = reqwest::Url::parse(&self.client.api_url("/api/v2.0/users"))
            .map_err(|e| AppError::BadRequest(format!("Invalid Harbor URL: {}", e)))?;

        let response = self
            .client
            .client
            .post(url)
            .headers(self.client.default_headers())
            .json(req)
            .send()
            .await
            .map_err(|e| AppError::BadRequest(format!("Harbor request failed: {}", e)))?;

        self.check_response(response).await
    }

    pub async fn delete_user(&self, username: &str) -> Result<(), AppError> {
        self.ensure_enabled()?;
        // List all Harbor users to find the user_id for this username
        let list_url = reqwest::Url::parse(&self.client.api_url("/api/v2.0/users?page=1&page_size=100"))
            .map_err(|e| AppError::BadRequest(format!("Invalid Harbor URL: {}", e)))?;

        let response = self
            .client
            .client
            .get(list_url)
            .headers(self.client.default_headers())
            .send()
            .await
            .map_err(|e| AppError::BadRequest(format!("Harbor request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(Self::map_harbor_error(status, &body));
        }

        let users: Vec<HarborUser> = response.json().await.map_err(|e| {
            AppError::BadRequest(format!("Failed to parse Harbor users: {}", e))
        })?;

        // Find the user by username
        if let Some(user) = users.iter().find(|u| u.username == username) {
            let delete_url = reqwest::Url::parse(&self.client.api_url(
                &format!("/api/v2.0/users/{}", user.user_id)
            ))
            .map_err(|e| AppError::BadRequest(format!("Invalid Harbor URL: {}", e)))?;

            let delete_resp = self
                .client
                .client
                .delete(delete_url)
                .headers(self.client.default_headers())
                .send()
                .await
                .map_err(|e| AppError::BadRequest(format!("Harbor request failed: {}", e)))?;

            self.check_response(delete_resp).await
        } else {
            // User doesn't exist in Harbor, that's OK
            Ok(())
        }
    }

    pub async fn create_project(&self, req: CreateProjectRequest) -> Result<(), AppError> {
        self.ensure_enabled()?;
        let url = reqwest::Url::parse(&self.client.api_url("/api/v2.0/projects"))
            .map_err(|e| AppError::BadRequest(format!("Invalid Harbor URL: {}", e)))?;

        let response = self
            .client
            .client
            .post(url)
            .headers(self.client.default_headers())
            .json(&req)
            .send()
            .await
            .map_err(|e| AppError::BadRequest(format!("Harbor request failed: {}", e)))?;

        self.check_response(response).await
    }

    pub async fn delete_project(&self, project_name: &str) -> Result<(), AppError> {
        self.ensure_enabled()?;
        let path = format!("/api/v2.0/projects/{}", project_name);
        let url = reqwest::Url::parse(&self.client.api_url(&path))
            .map_err(|e| AppError::BadRequest(format!("Invalid Harbor URL: {}", e)))?;

        let response = self
            .client
            .client
            .delete(url)
            .headers(self.client.default_headers())
            .send()
            .await
            .map_err(|e| AppError::BadRequest(format!("Harbor request failed: {}", e)))?;

        self.check_response(response).await
    }

    pub async fn delete_repository(
        &self,
        project_name: &str,
        repo_name: &str,
    ) -> Result<(), AppError> {
        self.ensure_enabled()?;
        let encoded_repo_name = urlencoding::encode(repo_name);
        let path = format!("/api/v2.0/projects/{}/repositories/{}", project_name, encoded_repo_name);
        let url = reqwest::Url::parse(&self.client.api_url(&path))
            .map_err(|e| AppError::BadRequest(format!("Invalid Harbor URL: {}", e)))?;

        let response = self
            .client
            .client
            .delete(url)
            .headers(self.client.default_headers())
            .send()
            .await
            .map_err(|e| AppError::BadRequest(format!("Harbor request failed: {}", e)))?;

        self.check_response(response).await
    }

    pub async fn delete_artifact(
        &self,
        project_name: &str,
        repo_name: &str,
        reference: &str,
    ) -> Result<(), AppError> {
        self.ensure_enabled()?;
        let encoded_repo_name = urlencoding::encode(repo_name);
        let path = format!(
            "/api/v2.0/projects/{}/repositories/{}/artifacts/{}",
            project_name, encoded_repo_name, reference
        );
        let url = reqwest::Url::parse(&self.client.api_url(&path))
            .map_err(|e| AppError::BadRequest(format!("Invalid Harbor URL: {}", e)))?;

        let response = self
            .client
            .client
            .delete(url)
            .headers(self.client.default_headers())
            .send()
            .await
            .map_err(|e| AppError::BadRequest(format!("Harbor request failed: {}", e)))?;

        self.check_response(response).await
    }

    pub async fn add_member(
        &self,
        project_name: &str,
        req: CreateMemberRequest,
    ) -> Result<(), AppError> {
        self.ensure_enabled()?;
        let path = format!("/api/v2.0/projects/{}/members", project_name);
        let url = reqwest::Url::parse(&self.client.api_url(&path))
            .map_err(|e| AppError::BadRequest(format!("Invalid Harbor URL: {}", e)))?;

        let response = self
            .client
            .client
            .post(url)
            .headers(self.client.default_headers())
            .json(&req)
            .send()
            .await
            .map_err(|e| AppError::BadRequest(format!("Harbor request failed: {}", e)))?;

        self.check_response(response).await
    }

    pub async fn update_password(&self, username: &str, new_password: &str) -> Result<(), AppError> {
        self.ensure_enabled()?;
        // List Harbor users to find the user_id
        let list_url = reqwest::Url::parse(&self.client.api_url("/api/v2.0/users?page=1&page_size=100"))
            .map_err(|e| AppError::BadRequest(format!("Invalid Harbor URL: {}", e)))?;

        let response = self
            .client
            .client
            .get(list_url)
            .headers(self.client.default_headers())
            .send()
            .await
            .map_err(|e| AppError::BadRequest(format!("Harbor request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(Self::map_harbor_error(status, &body));
        }

        let users: Vec<HarborUser> = response.json().await.map_err(|e| {
            AppError::BadRequest(format!("Failed to parse Harbor users: {}", e))
        })?;

        if let Some(harbor_user) = users.iter().find(|u| u.username == username) {
            let pwd_url = reqwest::Url::parse(&self.client.api_url(
                &format!("/api/v2.0/users/{}/password", harbor_user.user_id)
            ))
            .map_err(|e| AppError::BadRequest(format!("Invalid Harbor URL: {}", e)))?;

            let req = ChangePasswordRequest {
                // Use a different value for old_password since Harbor rejects
                // requests where old and new passwords are identical
                old_password: format!("{}_old", new_password),
                new_password: new_password.to_string(),
            };

            let pwd_resp = self
                .client
                .client
                .put(pwd_url)
                .headers(self.client.default_headers())
                .json(&req)
                .send()
                .await
                .map_err(|e| AppError::BadRequest(format!("Harbor request failed: {}", e)))?;

            self.check_response(pwd_resp).await
        } else {
            // User doesn't exist in Harbor, that's OK
            Ok(())
        }
    }

    pub async fn remove_member(&self, project_name: &str, member_id: i64) -> Result<(), AppError> {
        self.ensure_enabled()?;
        let path = format!("/api/v2.0/projects/{}/members/{}", project_name, member_id);
        let url = reqwest::Url::parse(&self.client.api_url(&path))
            .map_err(|e| AppError::BadRequest(format!("Invalid Harbor URL: {}", e)))?;

        let response = self
            .client
            .client
            .delete(url)
            .headers(self.client.default_headers())
            .send()
            .await
            .map_err(|e| AppError::BadRequest(format!("Harbor request failed: {}", e)))?;

        self.check_response(response).await
    }

    pub async fn list_registries(&self) -> Result<Vec<HarborRegistry>, AppError> {
        self.ensure_enabled()?;
        let url = reqwest::Url::parse(&self.client.api_url("/api/v2.0/registries?page=1&page_size=100"))
            .map_err(|e| AppError::BadRequest(format!("Invalid Harbor URL: {}", e)))?;

        let response = self
            .client
            .client
            .get(url)
            .headers(self.client.default_headers())
            .send()
            .await
            .map_err(|e| AppError::BadRequest(format!("Harbor request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(Self::map_harbor_error(status, &body));
        }

        response.json::<Vec<HarborRegistry>>().await.map_err(|e| {
            AppError::BadRequest(format!("Failed to parse Harbor registries: {}", e))
        })
    }

    pub async fn find_local_registry_endpoint(&self) -> Result<i64, AppError> {
        self.ensure_enabled()?;

        if let Some(id) = self.registry_endpoint_id {
            return Ok(id);
        }

        let registries = self.list_registries().await?;
        let base_url = self.normalize_registry_url(&self.client.base_url);

        for registry in &registries {
            let registry_url = self.normalize_registry_url(&registry.url);
            if registry_url == base_url {
                return Ok(registry.id);
            }
        }

        self.create_local_registry_endpoint().await?;

        let registries = self.list_registries().await?;
        for registry in &registries {
            let registry_url = self.normalize_registry_url(&registry.url);
            if registry_url == base_url {
                return Ok(registry.id);
            }
        }

        Err(AppError::BadRequest(
            "Failed to auto-create Harbor registry endpoint for the local instance. Please create one manually in Harbor or set registry_endpoint_id in config.toml.".to_string(),
        ))
    }

    async fn create_local_registry_endpoint(&self) -> Result<(), AppError> {
        let base_url = self.client.base_url.trim_end_matches('/').to_string();
        let insecure = self.registry_insecure.unwrap_or_else(|| self.is_local_registry_insecure(&base_url));
        let name = format!("local-harbor-{}", uuid::Uuid::new_v4());
        let req = CreateRegistryRequest {
            name: name.clone(),
            url: base_url.clone(),
            registry_type: "harbor".to_string(),
            credential: RegistryCredential {
                credential_type: "basic".to_string(),
                access_key: self.client.username.clone(),
                access_secret: self.client.password.clone(),
            },
            insecure,
            description: Some("Auto-created local Harbor endpoint for replication".to_string()),
        };

        let url = reqwest::Url::parse(&self.client.api_url("/api/v2.0/registries"))
            .map_err(|e| AppError::BadRequest(format!("Invalid Harbor URL: {}", e)))?;

        let response = self
            .client
            .client
            .post(url)
            .headers(self.client.default_headers())
            .json(&req)
            .send()
            .await
            .map_err(|e| AppError::BadRequest(format!("Harbor request failed: {}", e)))?;

        self.check_response(response).await
    }

    fn normalize_registry_url(&self, url: &str) -> String {
        let url = url.trim().trim_end_matches('/');
        let url = url.trim_start_matches("http://").trim_start_matches("https://");
        url.to_lowercase()
    }

    fn is_local_registry_insecure(&self, base_url: &str) -> bool {
        let lower = base_url.to_lowercase();
        if lower.starts_with("http://") {
            return true;
        }
        if let Some(host_part) = lower.strip_prefix("https://") {
            let host = host_part.split(':').next().unwrap_or(host_part);
            return host == "localhost" || host == "127.0.0.1" || host == "::1";
        }
        false
    }

    /// Harbor create endpoints (e.g. replication policies/executions) return 201 with an empty
    /// body and the resource URL in the `Location` header. Extract the trailing numeric ID.
    fn extract_created_id(response: &reqwest::Response, resource_name: &str) -> Result<i64, AppError> {
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|h| h.to_str().ok())
            .ok_or_else(|| {
                AppError::BadRequest(format!(
                    "Harbor did not return a Location header for the created {}",
                    resource_name
                ))
            })?;

        let id = location
            .split('/')
            .next_back()
            .unwrap_or("")
            .parse::<i64>()
            .map_err(|_| {
                AppError::BadRequest(format!(
                    "Invalid Location header from Harbor: {}",
                    location
                ))
            })?;

        Ok(id)
    }

    pub async fn create_replication_policy(
        &self,
        req: &CreateReplicationPolicyRequest,
    ) -> Result<ReplicationPolicy, AppError> {
        self.ensure_enabled()?;
        let url = reqwest::Url::parse(&self.client.api_url("/api/v2.0/replication/policies"))
            .map_err(|e| AppError::BadRequest(format!("Invalid Harbor URL: {}", e)))?;

        let response = self
            .client
            .client
            .post(url)
            .headers(self.client.default_headers())
            .json(req)
            .send()
            .await
            .map_err(|e| AppError::BadRequest(format!("Harbor request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(Self::map_harbor_error(status, &body));
        }

        // Harbor returns 201 with an empty body; the new policy ID is in the Location header.
        let id = Self::extract_created_id(&response, "replication policy")?;
        Ok(ReplicationPolicy {
            id,
            name: req.name.clone(),
        })
    }

    pub async fn delete_replication_policy(&self, policy_id: i64) -> Result<(), AppError> {
        self.ensure_enabled()?;
        let path = format!("/api/v2.0/replication/policies/{}", policy_id);
        let url = reqwest::Url::parse(&self.client.api_url(&path))
            .map_err(|e| AppError::BadRequest(format!("Invalid Harbor URL: {}", e)))?;

        let response = self
            .client
            .client
            .delete(url)
            .headers(self.client.default_headers())
            .send()
            .await
            .map_err(|e| AppError::BadRequest(format!("Harbor request failed: {}", e)))?;

        self.check_response(response).await
    }

    pub async fn trigger_replication(&self, policy_id: i64) -> Result<ReplicationExecution, AppError> {
        self.ensure_enabled()?;
        let url = reqwest::Url::parse(&self.client.api_url("/api/v2.0/replication/executions"))
            .map_err(|e| AppError::BadRequest(format!("Invalid Harbor URL: {}", e)))?;

        let req = ReplicationExecutionRequest { policy_id };

        let response = self
            .client
            .client
            .post(url)
            .headers(self.client.default_headers())
            .json(&req)
            .send()
            .await
            .map_err(|e| AppError::BadRequest(format!("Harbor request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(Self::map_harbor_error(status, &body));
        }

        // Harbor returns 201 with an empty body; the new execution ID is in the Location header.
        let id = Self::extract_created_id(&response, "replication execution")?;
        Ok(ReplicationExecution {
            id,
            policy_id,
            status: "InProgress".to_string(),
        })
    }

    pub async fn get_replication_execution(&self, execution_id: i64) -> Result<ReplicationExecution, AppError> {
        self.ensure_enabled()?;
        let path = format!("/api/v2.0/replication/executions/{}", execution_id);
        let url = reqwest::Url::parse(&self.client.api_url(&path))
            .map_err(|e| AppError::BadRequest(format!("Invalid Harbor URL: {}", e)))?;

        let response = self
            .client
            .client
            .get(url)
            .headers(self.client.default_headers())
            .send()
            .await
            .map_err(|e| AppError::BadRequest(format!("Harbor request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(Self::map_harbor_error(status, &body));
        }

        response.json::<ReplicationExecution>().await.map_err(|e| {
            AppError::BadRequest(format!("Failed to parse Harbor replication execution: {}", e))
        })
    }

    pub async fn wait_for_replication_execution(&self, execution_id: i64) -> Result<ReplicationExecution, AppError> {
        let timeout = std::time::Duration::from_secs(self.replication_timeout_secs);
        let interval = std::time::Duration::from_secs(self.replication_poll_interval_secs);
        let start = std::time::Instant::now();

        loop {
            let execution = self.get_replication_execution(execution_id).await?;
            match execution.status.as_str() {
                "Succeed" => return Ok(execution),
                "Failed" => {
                    return Err(AppError::BadRequest(format!(
                        "Replication execution {} failed",
                        execution_id
                    )));
                }
                "Stopped" => {
                    return Err(AppError::BadRequest(format!(
                        "Replication execution {} was stopped",
                        execution_id
                    )));
                }
                _ => {}
            }

            if start.elapsed() >= timeout {
                return Err(AppError::BadRequest(format!(
                    "Replication execution {} timed out after {} seconds; execution may still be running",
                    execution_id, self.replication_timeout_secs
                )));
            }

            tokio::time::sleep(interval).await;
        }
    }

    pub async fn replicate_artifact(
        &self,
        src_project: &str,
        dest_project: &str,
        repository_name: &str,
        tag: &str,
    ) -> Result<(), AppError> {
        self.ensure_enabled()?;

        let registry_id = self.find_local_registry_endpoint().await?;
        let short_repo = repository_name
            .split('/')
            .next_back()
            .unwrap_or(repository_name);

        let policy_name = format!("temp-approve-{}-{}", src_project, uuid::Uuid::new_v4());
        let req = CreateReplicationPolicyRequest {
            name: policy_name.clone(),
            description: Some(format!("Temporary policy to approve {}/{}", repository_name, tag)),
            src_registry: None,
            dest_registry: Some(RegistryEntity { id: registry_id }),
            dest_namespace: dest_project.to_string(),
            trigger: ReplicationTrigger {
                trigger_settings: None,
                trigger_type: "manual".to_string(),
            },
            filters: vec![
                ReplicationFilter {
                    filter_type: "name".to_string(),
                    value: format!("{}/{}", src_project, short_repo),
                },
                ReplicationFilter {
                    filter_type: "tag".to_string(),
                    value: tag.to_string(),
                },
                ReplicationFilter {
                    filter_type: "resource".to_string(),
                    value: "artifact".to_string(),
                },
            ],
            enabled: true,
            deletion: false,
            override_: true,
        };

        let policy = self.create_replication_policy(&req).await?;

        let execution = match self.trigger_replication(policy.id).await {
            Ok(execution) => execution,
            Err(error) => {
                if let Err(cleanup_error) = self.delete_replication_policy(policy.id).await {
                    tracing::warn!(
                        "Failed to delete replication policy {} after trigger failure: {}",
                        policy.id,
                        cleanup_error
                    );
                }
                return Err(error);
            }
        };

        match self.wait_for_replication_execution(execution.id).await {
            Ok(_) => {
                let _ = self.delete_replication_policy(policy.id).await;
                Ok(())
            }
            Err(e) => {
                tracing::warn!("Replication policy {} will not be deleted automatically because execution {} status is uncertain", policy.id, execution.id);
                Err(e)
            }
        }
    }
}
