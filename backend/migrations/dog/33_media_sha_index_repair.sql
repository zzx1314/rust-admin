-- @migration-name: media_sha_index_repair

-- 修索引（补跑版）。
--
-- 迁移 32 首次发布时带了 `@if-not-exists: column dog_media.sha256` 前置条件，
-- 而 sha256 列在迁移 31 就已经存在 → 整个文件被跳过、迁移却记成「已执行」，
-- 正式库里的 partial index（`WHERE ...`）一直没换成普通唯一索引，
-- CentOS 7 的 sqlite3 3.7.17 因此连 `pragma table_info` 都读不了。
--
-- 这里把同一段幂等 SQL 无条件再跑一次（32 已经修好的库执行结果一样）：
-- 先把历史重复行的 sha256 置空（保留数据、只丢去重键），再换普通唯一索引。
UPDATE dog_media
SET sha256 = NULL
WHERE sha256 IS NOT NULL
  AND id NOT IN (
      SELECT MAX(id) FROM dog_media WHERE sha256 IS NOT NULL GROUP BY sha256
  );

DROP INDEX IF EXISTS uq_dog_media_sha256;

CREATE UNIQUE INDEX IF NOT EXISTS uq_dog_media_sha256 ON dog_media(sha256);
