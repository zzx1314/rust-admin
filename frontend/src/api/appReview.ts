import { http } from "@/utils/http";

export interface AppReview {
  id: number;
  srcProject: string;
  destProject: string;
  repositoryName: string;
  tag: string;
  digest?: string;
  artifactId?: number;
  status: "pending" | "approved" | "rejected";
  reviewerComment?: string;
  createdBy?: number;
  reviewerId?: number;
  createTime: string;
  updateTime?: string;
}

export interface AppReviewPageData {
  records: AppReview[];
  total: number;
  current: number;
  size: number;
}

export interface CreateAppReviewRequest {
  srcProject: string;
  destProject: string;
  repositoryName: string;
  tag: string;
  digest?: string;
  artifactId?: number;
  reviewerComment?: string;
}

export interface ReviewActionRequest {
  reviewerComment?: string;
}

type Result<T = any> = {
  code: number;
  msg: string;
  data?: T;
};

export const createAppReview = (data: CreateAppReviewRequest) => {
  return http.axiosPostRequest<Result<AppReview>>("/api/appReviews", data);
};

export const getAppReviews = (params?: {
  srcProject?: string;
  repositoryName?: string;
  status?: string;
  current?: number;
  size?: number;
}) => {
  return http.axiosGetRequest<Result<AppReviewPageData>>("/api/appReviews", {
    ...params
  });
};

export const getAppReview = (id: number) => {
  return http.axiosGetRequest<Result<AppReview>>(`/api/appReviews/${id}`, {});
};

export const approveAppReview = (id: number, data?: ReviewActionRequest) => {
  return http.axiosPostRequest<Result<AppReview>>(
    `/api/appReviews/${id}/approve`,
    data || {}
  );
};

export const rejectAppReview = (id: number, data?: ReviewActionRequest) => {
  return http.axiosPostRequest<Result<AppReview>>(
    `/api/appReviews/${id}/reject`,
    data || {}
  );
};

export const deleteAppReview = (id: number) => {
  return http.axiosDelete<Result>(`/api/appReviews/${id}`);
};
