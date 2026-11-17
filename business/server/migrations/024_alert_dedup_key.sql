-- 告警 / 自动工单的去重键（2026-11-17 现场 TK-1112-11 复盘后加）。
--
-- 原来「是不是同一件事」的判据是**工单标题完全相等**，而标题 = 「设备号 + 告警内容前
-- 60 字」（`alert_flow::work_order_title`）。告警内容里带易变字段：
--   * AI 检出的内容带置信度（0.31 / 0.42 …），任务名/航点名叫法短时置信度会进标题；
--   * AI 数量核对的内容带证据图路径（detect_jobs/<plan>/pt<n>/…）；
-- 于是每跑一轮巡检标题都不一样，24h 去重形同虚设，一张工单变一堆。
--
-- 现在由告警来源自己给出结构化键（键里的 `device` 是设备编号字符串），工单按「同一设备
-- + 同一键」去重（`alert_flow::find_open_order`）：
--   ai:<device>:<taskCode>:<point>:<label>   AI 检出
--   ai:<device>:<taskCode>:<point>:count     AI 数量核对不一致
--   ai:<device>:<taskCode>:<point>:missing   AI 未检出（exist + alertWhenMissing）
--   fault:<device>:<code>                    本体故障码（4021 message 里的 code=）
--   hb:<device>                              心跳超时
--   ctrl:<device>                            控制权被抢占（ctrl_source=3）
-- 空值 = 该来源没给键（人工建的告警等），退回「标题相等」的老行为，不影响历史数据。

ALTER TABLE alerts ADD COLUMN dedup_key TEXT;
ALTER TABLE work_orders ADD COLUMN dedup_key TEXT;

CREATE INDEX IF NOT EXISTS idx_alerts_dedup ON alerts(dedup_key);
CREATE INDEX IF NOT EXISTS idx_work_orders_dedup ON work_orders(dedup_key, status);
