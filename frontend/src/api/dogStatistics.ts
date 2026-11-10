import { http } from "@/utils/http";

type Result = { code: number; msg: string; data?: Array<any> };
type ResultOne = { code: number; msg: string; data?: any };

const dogStatisticsUrls = {
  all: `/api/dogStatistics/all`,
  allType: `/api/dogStatistics/allType`
};

export const statisticsAll = (query?: object) =>
  http.axiosGetRequest<Result>(dogStatisticsUrls.all, query);
export const statisticsAllType = (param?: number) =>
  http.axiosGetRequest<ResultOne>(dogStatisticsUrls.allType, param);
