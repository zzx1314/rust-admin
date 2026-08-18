# SQL migrations

迁移器会自动扫描本目录下所有 `.sql` 文件，不再需要在 `src/migration` 中为每个文件新增 Rust 模块。

## 文件规则

文件名必须使用以下格式：

```text
<数字版本>_<描述>.sql
```

例如：

```text
6_add_audit_log.sql
```

迁移器按数字版本升序执行；版本相同的文件再按相对路径排序。新的迁移默认名称为 `m<六位版本>_<描述>`。

## 文件指令

SQL 文件可以通过注释声明规则：

```sql
-- @migration-name: add_user_is_edit
-- @if-not-exists: column p_sys_user.is_edit
```

支持的指令：

- `@migration-name`：显式指定迁移名称。多个 SQL 文件使用同一名称时，会合并为一个 SeaORM 迁移，适合兼容历史迁移名称。
- `@if-not-exists: column <table>.<column>`：当指定表中已经存在字段时跳过当前 SQL 文件。

指令必须位于 SQL 注释中，SQL 内容仍由文件本身负责。迁移名称一旦用于生产环境，不应随意修改，否则会被识别为新的迁移。
