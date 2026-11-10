-- @migration-name: drop_command_services_tables

-- 指令 / 服务业务不再由中台维护，直接删除表（不做软删除）。

DROP TABLE IF EXISTS dog_command;
DROP TABLE IF EXISTS dog_services;
