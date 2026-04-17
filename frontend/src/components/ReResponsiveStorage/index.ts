import type { StorageOpts, StorageType } from "./types";
import { reactive } from "vue";

export default class Storage {
  static _nameSpace = "rs-";
  static _storageType: StorageType = "localStorage"; // 默认使用 localStorage

  static _getStaticKey = (nameSpace: string, key: string) =>
    `${nameSpace ?? this._nameSpace}${key}`;

  static install(app: any, options: StorageOpts) {
    const {
      nameSpace = this._nameSpace,
      memory,
      storageType = this._storageType
    } = options;
    this._storageType = storageType; // 设置当前存储类型
    memory && this.clearAll(nameSpace, memory);
    return new Storage(app, options);
  }

  static clearAll(nameSpace: string, memory: object) {
    Object.keys(memory).forEach(key => {
      const alias: string = nameSpace + key;
      const storage =
        this._storageType === "localStorage"
          ? window.localStorage
          : window.sessionStorage;
      if (Object.prototype.hasOwnProperty.call(storage, alias)) {
        storage.removeItem(alias);
      }
    });
  }

  static get(key: string) {
    const storage =
      this._storageType === "localStorage"
        ? window.localStorage
        : window.sessionStorage;
    return JSON.parse(storage.getItem(key) as string);
  }

  static set(key: string, val: any) {
    const storage =
      this._storageType === "localStorage"
        ? window.localStorage
        : window.sessionStorage;
    val = typeof val === "object" ? JSON.stringify(val) : val;
    storage.setItem(key, val);
  }

  static getData(key: string, nameSpace?: string) {
    const storage =
      this._storageType === "localStorage"
        ? window.localStorage
        : window.sessionStorage;
    const fullKey = this._getStaticKey(nameSpace!, key);
    if (Object.prototype.hasOwnProperty.call(storage, fullKey)) {
      return JSON.parse(storage.getItem(fullKey) as string);
    }
  }

  public constructor(app: any, options: StorageOpts) {
    const that = Storage;
    const { version = 3, nameSpace = that._nameSpace, memory } = options;
    const _getKey = (key: string): string => nameSpace + key;

    /**
     * Vue2 uses defineReactive to create responsive storage
     * Vue3 uses reactive to create responsive storage
     */
    const _storage: any = version === 3 ? reactive(memory) : memory;

    if (Object.keys(_storage).length === 0) console.warn("key cannot be empty");

    Object.keys(_storage).forEach(key => {
      const val = _storage[key];
      that.set(_getKey(key), val);

      Reflect.defineProperty(_storage, key, {
        get: () => that.get(_getKey(key)),
        set: val => that.set(_getKey(key), val),
        configurable: true
      });

      if (version === 2) app.util.defineReactive(_storage, key, _storage[key]);
    });

    const _target = version === 3 ? app.config.globalProperties : app.prototype;
    Reflect.defineProperty(_target, "$storage", {
      get: () => _storage
    });
  }
}
