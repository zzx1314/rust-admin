-- 为园区、区域、路线、任务增加创建人字段
-- 用于实现「每个创建人只能看到自己创建的数据」
ALTER TABLE parks ADD COLUMN created_by INTEGER REFERENCES users(id);
ALTER TABLE zones ADD COLUMN created_by INTEGER REFERENCES users(id);
ALTER TABLE routes ADD COLUMN created_by INTEGER REFERENCES users(id);
ALTER TABLE tasks ADD COLUMN created_by INTEGER REFERENCES users(id);
