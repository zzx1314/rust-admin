import { http } from "@/utils/http";

type Result<T = any> = {
  code: number;
  msg: string;
  data?: T;
};

export interface PaginatedData<T> {
  records: T[];
  total: number;
  current: number;
  size: number;
}

export interface HarborProject {
  project_id: number;
  owner_id: number;
  name: string;
  owner_name?: string;
  repo_count?: number;
  current_user_role_id?: number;
  current_user_role_ids?: number[];
  creation_time?: string;
  update_time?: string;
  metadata?: Record<string, string>;
  registry_id?: number;
}

export interface ProjectSummary {
  repo_count: number;
  project_admin_count?: number;
  maintainer_count?: number;
  developer_count?: number;
  guest_count?: number;
  limited_guest_count?: number;
  quota?: any;
  registry?: any;
}

export interface HarborRepository {
  id: number;
  project_id: number;
  name: string;
  description?: string;
  artifact_count?: number;
  pull_count?: number;
  creation_time?: string;
  update_time?: string;
}

export interface HarborStatistics {
  total_projects: number;
  total_repositories: number;
  total_artifacts: number;
  total_pull_count: number;
  public_project_count: number;
  private_project_count: number;
  top_repositories: RepoStat[];
  recent_projects: HarborProject[];
}

export interface RepoStat {
  name: string;
  project_name: string;
  pull_count: number;
  artifact_count: number;
}

export interface ArtifactTag {
  name: string;
  push_time?: string;
  pull_time?: string;
  signed?: boolean;
}

export interface HarborArtifact {
  id: number;
  digest?: string;
  size?: number;
  push_time?: string;
  pull_time?: string;
  tags?: ArtifactTag[];
  manifest_media_type?: string;
  media_type?: string;
}

export interface HarborMember {
  id: number;
  project_id: number;
  entity_name: string;
  role_name: string;
  role_id: number;
  entity_id: number;
  entity_type: string;
}

const projectUrl = "/api/harbor/projects";

export const getHarborStatistics = () => {
  return http.axiosGetRequest<Result<HarborStatistics>>(
    "/api/harbor/statistics",
    {}
  );
};

export const listProjects = (query?: {
  name?: string;
  public?: boolean;
  page?: number;
  page_size?: number;
}) => {
  return http.axiosGetRequest<Result<PaginatedData<HarborProject>>>(
    projectUrl,
    query
  );
};

export const createProject = (data: {
  project_name: string;
  metadata: Record<string, string>;
}) => {
  return http.axiosPostRequest<Result>(projectUrl, data);
};

export const deleteProject = (projectName: string) => {
  return http.axiosDelete<Result>(`${projectUrl}/${projectName}`);
};

export const getProjectSummary = (projectName: string) => {
  return http.axiosGetRequest<Result<ProjectSummary>>(
    `${projectUrl}/${projectName}/summary`
  );
};

export const listArtifacts = (
  projectName: string,
  repoName: string,
  query?: { page?: number; page_size?: number }
) => {
  return http.axiosGetRequest<Result<PaginatedData<HarborArtifact>>>(
    `${projectUrl}/${projectName}/artifacts`,
    { ...query, repo_name: repoName }
  );
};

export const listRepositories = (
  projectName: string,
  query?: { page?: number; page_size?: number }
) => {
  return http.axiosGetRequest<Result<PaginatedData<HarborRepository>>>(
    `${projectUrl}/${projectName}/repositories`,
    query
  );
};

export const listMembers = (
  projectName: string,
  query?: { page?: number; page_size?: number }
) => {
  return http.axiosGetRequest<Result<PaginatedData<HarborMember>>>(
    `${projectUrl}/${projectName}/members`,
    query
  );
};

export const addMember = (
  projectName: string,
  data: {
    role_id: number;
    member_user: { user_id?: number; username?: string };
  }
) => {
  return http.axiosPostRequest<Result>(
    `${projectUrl}/${projectName}/members`,
    data
  );
};

export const removeMember = (projectName: string, memberId: number) => {
  return http.axiosDelete<Result>(
    `${projectUrl}/${projectName}/members/${memberId}`
  );
};

export interface HarborInfo {
  registry_url: string;
  enabled: boolean;
}

export const getHarborInfo = () => {
  return http.axiosGetRequest<Result<HarborInfo>>("/api/harbor/info", {});
};
