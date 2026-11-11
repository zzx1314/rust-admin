-- 中台账号登录与地图归属
--
-- 业务平台的登录改为「中台账号优先」：
--   * 中台账号登录成功后，本地 upsert 一条 users 记录，并用 zentai_user_id 关联；
--   * 纯本地服务账号（设备客户端使用，zentai_user_id 为空）仍走 bcrypt 本地校验。
--
-- 地图版本记录中台 dog_map.user_id，园区选择地图时按当前登录用户的
-- zentai_user_id 过滤，保证只能看到自己名下机器狗采集的地图。

ALTER TABLE users ADD COLUMN zentai_user_id INTEGER;
ALTER TABLE users ADD COLUMN zentai_username TEXT;
ALTER TABLE users ADD COLUMN zentai_roles TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS uq_users_zentai_user ON users(zentai_user_id);

ALTER TABLE map_versions ADD COLUMN owner_user_id INTEGER;
ALTER TABLE map_versions ADD COLUMN owner_username TEXT;
