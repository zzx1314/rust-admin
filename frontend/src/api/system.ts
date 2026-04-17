import { http } from "@/utils/http";
import type internal from "stream";

type Result = {
  code: number;
  msg: string;
  data?: Array<any>;
};

type ResultOne = {
  code: number;
  msg: string;
  data?: any;
};

type ResultPage = {
  code: number;
  msg: string;
  data?: {
    records: Array<any>;
    total: number;
  };
};

const orgurls = {
  allList: `/api/sysOrg/allList`,
  saveSysOrg: `/api/sysOrg`,
  updateById: `/api/sysOrg`,
  removeById: `/api/sysOrg/`,
  removeByIds: `/api/sysOrg/removeByIds`
};

const userUrls = {
  userPage: `/api/sysUser/getPage`,
  saveUser: "/api/sysUser",
  deleteUserById: "/api/sysUser/",
  update: "/api/sysUser",
  resetPwd: "/api/sysUser/resetPwd",
  userInfo: `/api/sysUser/info`,
  enable: `/api/sysUser/enable`,
  getUserByRoleId: `/api/sysUser/getUserByRoleId/`
};

const roleUrls = {
  rolePage: `/api/sysRole/getPage`,
  listAll: "/api/sysRole/getAll",
  save: "/api/sysRole",
  update: "/api/sysRole",
  deleteById: "/api/sysRole/",
  listNoLog: `/api/sysRole/listNoLog`
};

const menuUrls = {
  menuPage: `/api/sysMenu/getAll`,
  saveSysMenu: `/api/sysMenu`,
  updateSysMenuById: `/api/sysMenu`,
  deleteSysMenuById: `/api/sysMenu/`,
  getSysMenuByRoleId: `/api/sysMenu/`
};

const authUrls = {
  getMenuData: `/api/sysAuth/getMenuData/`,
  getRoleData: `/api/sysAuth/getRoleData`,
  setRoleAuth: `/api/sysAuth/setRoleAuth`,
  setMenuAuth: `/api/sysAuth/setMenuAuth`
};

const fileUploadUrls = {
  taskInfo: `/api/minio/tasks/`,
  initTask: `/api/minio/tasks/init`,
  preSignUrl: `/api/minio/tasks/preSignUrl/`,
  merge: `/api/minio/tasks/merge/`
};

const fileMinoUp = {
  checkFileByMd5: `/api/files/multipart/check/`,
  initMultiPartUpload: "/api/files/multipart/init",
  mergeMultipartUpload: "/api/files/multipart/merge/",
  downloadMultipartFile: "/api/files/downloadByFileId/",
  getFileList: "/api/files/list"
};

const dictUrls = {
  pageItem: `/api/sysDictItem/getPage`,
  pageDict: `/api/sysDict/page`,
  getItemById: `/api/sysDictItem/getDictItemByDictId/`,
  saveItem: `/api/sysDictItem/save`,
  save: `/api/sysDict/save`,
  updateItem: `/api/sysDictItem/update`,
  update: `/api/sysDict/update`,
  deleteIte: `/api/sysDictItem/deleteItem/`,
  delete: `/api/sysDict/deleteDict/`,
  getSafePolicy: `/api/sysDict/getSafePolicy`,
  updateSafePolicy: `/api/sysDictItem/updateSafePolicy`,
  getDictItemByType: `/api/sysDictItem/getDIByDT/`,
  getSelectByType: `/api/sysDictItem/getSelectByType/`
};

/**
 * 获取字典项
 */
export const getDictItemByType = (query?: string) => {
  return http.axiosGetRequest<Result>(dictUrls.getDictItemByType + query, null);
};

/**
 * 获取字典项
 */
export const getSelectByType = (query?: string) => {
  return http.axiosGetRequest<Result>(dictUrls.getSelectByType + query, null);
};
/**
 * 获取字典项列表
 */
export const getDictPage = (query?: object) => {
  return http.axiosGetRequest<ResultPage>(dictUrls.pageDict, query);
};

/**
 * 通过id获取字典项
 */
export const getItemById = (id: string) => {
  return http.axiosGetRequest<ResultOne>(dictUrls.getItemById + id, null);
};

/**
 * 保存字典项
 */
export const saveItem = (data?: object) => {
  return http.axiosPostRequest<Result>(dictUrls.saveItem, data);
};

/**
 * 保存字典
 */
export const saveDict = (data?: object) => {
  return http.axiosPostRequest<Result>(dictUrls.save, data);
};

/**
 * 修改字典项
 */
export const updateItem = (data?: object) => {
  return http.axiosPut<Result>(dictUrls.updateItem, data);
};

/**
 * 修改字典
 */
export const updateDict = (data?: object) => {
  return http.axiosPut<Result>(dictUrls.update, data);
};

/**
 * 删除字典项
 */
export const deleteDictItem = (id: object) => {
  return http.axiosDelete<Result>(dictUrls.deleteIte + id);
};
export const deleteDict = (id: object) => {
  return http.axiosDelete<Result>(dictUrls.delete + id);
};

/**
 * 获取安全策略
 */
export const getSafePolicy = () => {
  return http.axiosGetRequest<ResultOne>(dictUrls.getSafePolicy, null);
};

/**
 * 修改安全策略
 */
export const updateSafePolicy = (data?: object) => {
  return http.axiosPut<Result>(dictUrls.updateSafePolicy, data);
};

/**
 * 检查文件
 */
export const checkFileByMd5 = (data: object, busstr: object) => {
  return http.axiosGetRequest<Result>(fileMinoUp.checkFileByMd5 + data, busstr);
};

type initMultPartFileResultType = {
  code: number;
  data: {
    urls: Array<any>;
  };
};
/** 初始化分块上传 */
export const initMultPartFile = (data?: object) => {
  return http.axiosPostRequest<initMultPartFileResultType>(
    fileMinoUp.initMultiPartUpload,
    data
  );
};

/** 合并文件 */
export const mergeFileByMd5 = (data?: object) => {
  return http.axiosPostRequest<Result>(fileMinoUp.mergeMultipartUpload + data);
};

type queryParam = {
  range: string;
  id: object;
};
/** 分块下载文件 */
export const chunkDownloadFile = (data: queryParam) => {
  return http.axiosGetDown(
    fileMinoUp.downloadMultipartFile + data.id,
    data.range
  );
};
/** 获取文件列表 */
export const fetchFileList = () => {
  return http.axiosGet<Result>(fileMinoUp.getFileList);
};

/** 获取上传任务信息 */
export const taskInfo = (data?: object) => {
  return http.axiosGet<Result>(fileUploadUrls.taskInfo + data);
};

/** 初始化上传任务 */
export const initTask = (data?: object) => {
  return http.axiosPostRequest<Result>(fileUploadUrls.initTask, data);
};

type upParam = {
  identifier: object;
  partNumber: number;
};

/** 获取预签名url */
export const preSignUrl = (data: upParam) => {
  return http.axiosGet<Result>(
    fileUploadUrls.preSignUrl + data.identifier + "/" + data.partNumber
  );
};

/** 合并文件 */
export const merge = (data?: object) => {
  return http.axiosPostRequest<Result>(fileUploadUrls.merge + data);
};

/** 获取部门管理列表 */
export const getDeptList = (data?: object) => {
  return http.axiosGetRequest<Result>(orgurls.allList, data);
};

/**
 * 保存部门
 */
export const saveSysOrg = (param?: object) => {
  return http.axiosPostRequest<Result>(orgurls.saveSysOrg, param);
};

/**
 * 修改部门
 */
export const updateById = (param?: object) => {
  return http.axiosPut<Result>(orgurls.updateById, param);
};

/**
 * 删除部门
 */
export const removeById = (param: internal) => {
  return http.axiosDelete<Result>(orgurls.removeById + param);
};

/**
 * 删除部门
 */
export const removeByIds = (param?: object) => {
  return http.axiosPostRequest<Result>(orgurls.removeByIds, param);
};

/**
 * 分页查询用户
 */
export const userPage = (query?: object) => {
  return http.axiosGetRequest<ResultPage>(userUrls.userPage, query);
};

/**
 * 重置用户
 */
export const userResetPwd = (param?: object) => {
  return http.axiosPut<Result>(userUrls.resetPwd, param);
};
/**
 * 禁用和启用
 */
export const userEnable = (param?: object) => {
  return http.axiosPut<Result>(userUrls.enable, param);
};

/**
 * 删除用户
 */
export const removeUserById = (param: internal) => {
  return http.axiosDelete<Result>(userUrls.deleteUserById + param);
};

/** 修改用户 */
export const updateUser = (param: object) => {
  return http.axiosPut<Result>(userUrls.update, param);
};

/**
 * 重置用户密码
 */
export const saveUser = (param?: object) => {
  return http.axiosPostRequest<Result>(userUrls.saveUser, param);
};

/**
 * 获取用户信息
 */
export const getUserInfo = () => {
  return http.axiosGetRequest<any>(userUrls.userInfo, {});
};

/**
 * 根据角色id获取用户
 */
export const getUserByRoleId = (roleId?: object) => {
  return http.axiosGetRequest<ResultPage>(
    userUrls.getUserByRoleId + roleId,
    null
  );
};

/** 获取角色管理列表 */
export const getRoleList = (query?: object) => {
  return http.axiosGetRequest<ResultPage>(roleUrls.rolePage, query);
};

/** 保存角色 */
export const saveRole = (param: object) => {
  return http.axiosPostRequest<Result>(roleUrls.save, param);
};

/** 修改角色 */
export const updateRole = (param: object) => {
  return http.axiosPut<Result>(roleUrls.update, param);
};

/** 删除角色 */
export const deleteRole = (param: object) => {
  return http.axiosDelete<Result>(roleUrls.deleteById + param, {});
};

/** 获取所有角色 */
export const listAllRole = () => {
  return http.axiosGetRequest<Result>(roleUrls.listAll, {});
};

/** 获取所有角色下拉菜单 */
export const getRoleSelectList = () => {
  return http.axiosGetRequest<Result>(roleUrls.listNoLog, {});
};

/** 菜单页面 */
export const menuPage = (query?: object) => {
  return http.axiosGet<Result>(menuUrls.menuPage, query);
};

/** 保存菜单 */
export const saveSysMenu = (param?: object) => {
  return http.axiosPostRequest<Result>(menuUrls.saveSysMenu, param);
};

/** 修改菜单 */
export const updateSysMenuById = (param?: object) => {
  return http.axiosPut<Result>(menuUrls.updateSysMenuById, param);
};

/**删除菜单 */
export const deleteSysMenu = (param?: object) => {
  return http.axiosDelete<Result>(menuUrls.getSysMenuByRoleId + param);
};

/** 获取权限菜单列表 */
export const getMenuData = (adminCode?: string) => {
  return http.axiosGetRequest<Result>(authUrls.getMenuData + adminCode, {});
};

/** 获取角色列表 */
export const getRoleData = (data?: object) => {
  return http.axiosGetRequest<Result>(authUrls.getRoleData, data);
};

/**设置角色权限 */
export const setRoleAuth = (param?: object) => {
  return http.axiosPostRequest<Result>(authUrls.setRoleAuth, param);
};

/**设置菜单权限 */
export const setMenuAuth = (param?: object) => {
  return http.axiosPostRequest<Result>(authUrls.setMenuAuth, param);
};
