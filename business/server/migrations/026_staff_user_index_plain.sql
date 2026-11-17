-- 修 025 的部分唯一索引：CentOS 7 自带 sqlite3 CLI（3.7.17）不认
-- `CREATE UNIQUE INDEX ... WHERE ...` 语法，读到这种索引就报
-- `malformed database schema (idx_staff_user)`，整库的 `.tables` / `pragma table_info`
-- 全都用不了（服务端链接自带的新 SQLite，所以线上功能正常，但运维排查会瞎）。
--
-- 这里换成普通索引；唯一性继续由应用层 `check_account_binding()` 保证
-- （单管理员编辑责任人库，没有并发抢绑的场景）。

DROP INDEX IF EXISTS idx_staff_user;
CREATE INDEX IF NOT EXISTS idx_staff_user ON staff(user_id);
