use crate::common::error::AppError;
use crate::common::pagination::PageResponse;
use crate::common::util::format_iso_datetime;
use crate::harbor::client::HarborClient;
use crate::harbor::models::{
    CreateMemberRequest, CreateProjectRequest, HarborMember, HarborProject, HarborRepository,
    ProjectQuery, ProjectSummary,
};
use std::sync::Arc;

pub struct HarborService {
    client: Arc<HarborClient>,
}

impl HarborService {
    pub fn new(client: Arc<HarborClient>) -> Self {
        Self { client }
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
}
