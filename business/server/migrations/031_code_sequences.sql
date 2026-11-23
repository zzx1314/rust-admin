-- 业务编号分配器。
--
-- 取代各建单接口里的 `SELECT COALESCE(MAX(id), 0) + 1 FROM <表>`：读和写不在同一条
-- 语句里，连接池 + WAL 下两个并发请求会拿到同一个值，第二个 INSERT 撞
-- `<表>_code ... UNIQUE` 直接 500。见 `src/ids.rs`。
--
-- `value` 是「已分配到的编号」，首次使用时用对应表的 `MAX(id)` 初始化，
-- 升级前后的编号保持连续。
CREATE TABLE IF NOT EXISTS code_sequences (
    name TEXT PRIMARY KEY,
    value INTEGER NOT NULL DEFAULT 0
);
