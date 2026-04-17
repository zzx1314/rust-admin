export type Version = 2 | 3;

export type StorageType = "localStorage" | "sessionStorage";

export interface StorageOpts {
  /** vue版本 可选2、3 默认3 */
  version?: Version;
  /** 命名空间 默认  `rs-` */
  nameSpace?: string;
  /** 需要存储的响应式对象 */
  memory: object;
  /** 存储数据 */
  storageType?: StorageType;
}
