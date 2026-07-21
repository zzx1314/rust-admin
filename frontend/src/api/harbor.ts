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

export const listProjects = (query?: {
  name?: string;
  public?: boolean;
  page?: number;
  page_size?: number;
}) => {
  return http.axiosGetRequest<Result<PaginatedData<HarborProject>>>(projectUrl, query);
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
