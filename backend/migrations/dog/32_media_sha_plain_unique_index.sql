-- @migration-name: media_sha_plain_unique_index

-- 把迁移 31 的 partial index 换成**普通唯一索引**。
--
-- 原因：`uq_dog_media_sha256 ... WHERE sha256 IS NOT NULL AND is_deleted = 0` 是
-- partial index（SQLite 3.8+）。正式环境（CentOS 7）自带的是 sqlite3 3.7.17，
-- 打开这个库会直接报 `malformed database schema ... near "WHERE"`，
-- 连 `pragma table_info` / `.tables` 都查不了 —— 现场排障全靠那个 CLI，不能为了
-- 一个索引把整个库变成「只有程序能读」。
--
-- 语义不变：SQLite 的唯一索引本来就把 NULL 视为互不相同，`sha256 IS NOT NULL`
-- 这半句是多余的；软删除的行由应用层在「同一张图再次回传」时恢复
-- （见 `DogMediaService::save_upload`），所以不需要把 is_deleted 写进索引条件。
-- ⚠️ 这个文件**不能**加 `@if-not-exists: column dog_media.sha256`：那是「列不存在才
-- 执行」的前置条件，而这一版是「列已经存在（迁移 31 加的）之后」才要修索引，
-- 加了就等于永远跳过（2026-11-20 首次发布踩过：索引没换成，sqlite3 CLI 仍报
-- malformed schema）。旧库由迁移 33 补同一段幂等 SQL。
DROP INDEX IF EXISTS uq_dog_media_sha256;

CREATE UNIQUE INDEX IF NOT EXISTS uq_dog_media_sha256 ON dog_media(sha256);
