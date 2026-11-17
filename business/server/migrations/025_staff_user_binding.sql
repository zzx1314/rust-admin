-- 责任人 ↔ 平台账号 绑定（2026-11-17）。
--
-- 现场问题：排班把告警/工单派给了「张泽新」（`staff`），但平台不知道张泽新登录的是哪个
-- 账号（`users`）——两者只有姓名/手机号，没有关联。于是：
--   * 做不到「只看派给我的」「待我处理 N」这类个人视图；
--   * 也没法把工单处置权限定给责任人本人（只能退化成「系统派单全员可见」）。
--
-- 绑定规则：
--   * 可空：老数据 / 没有平台账号的现场人员（如外包、临时工）保持 NULL，不影响派单；
--   * 一个账号最多绑一个责任人（唯一索引），否则「我的工单」不唯一；
--   * 责任人没绑账号时，系统派给他的单仍是**公共待办**（谁都能打开处置），
--     避免「派了单但没人能动」；一旦绑了账号，就只由他本人（+ 管理员）处置。
--
-- 登录侧说明：业务平台既支持本地账号，也支持中台 SSO（首次登录会 upsert 一条本地
-- `users` 行并签发 `sub = users.id` 的令牌），所以绑 `users.id` 两边都成立。

ALTER TABLE staff ADD COLUMN user_id INTEGER REFERENCES users(id);

-- 查询索引：「我的工单」按 `staff.user_id` 反查责任人。
--
-- 注意**不能**用「部分唯一索引」(`... WHERE user_id IS NOT NULL`)：
-- 目标机 CentOS 7 自带的 sqlite3 CLI 是 3.7.17（2013 年），不认部分索引语法，
-- 一碰到就报 `malformed database schema (idx_staff_user)`，连 `.tables` 都用不了
-- （服务端链接的是自带的较新 SQLite，所以服务端没事，运维侧全瞎）。
-- 「一个账号只绑一个责任人」由应用层 `check_account_binding()` 保证。
CREATE INDEX IF NOT EXISTS idx_staff_user ON staff(user_id);
