/**
 * 语音播报（喊话）内容的净化 + 校验。
 *
 * 三处必须同一份规则，改之前先对齐：
 *
 * ```text
 * 板端固件      backpack_dashboard/backend/speak.py::normalize
 * 设备客户端    client/src/speak.rs::sanitize_text
 * 平台前端      这里（路线编辑器「语音播报」+ 任务详情「呼叫」共用）
 * ```
 *
 * 板端会**默默**丢掉白名单之外的字符并截断（决不让一句话把任务搞乱），所以前端必须
 * 抢先拦下来：否则操作员填了带英文/空格的话，喊出来的却是另一句（或半句）。
 */
import { SPEAK_MAX_CHARS, SPEAK_MAX_LEN } from "./constants";

/** 板端认的中文标点（与 `speak.py::_PUNCT` 同一份） */
export const SPEAK_PUNCTUATIONS = "，。！？、；：（）《》〈〉「」『』【】…—·";

export interface SpeakTextResult {
  /** 净化后的内容；有 `error` 时为空串 */
  text: string;
  /** 非空 = 不能提交，直接展示给操作员 */
  error: string;
}

/**
 * 净化 + 校验播报内容（去空白、只留中文汉字 + 中文标点 + 数字）。
 *
 * `error` 非空时**不要**下发：播报是说给人听的，越界字符在板端被丢掉会变成半句话，
 * 宁可让操作员当场改。
 */
export function normalizeSpeakText(raw: string): SpeakTextResult {
  const compact = String(raw ?? "").replace(/\s+/g, "");
  if (!compact) return { text: "", error: "播报内容不能为空" };
  const bad = Array.from(compact).filter(ch => {
    const chinese = /[\u4e00-\u9fff]/.test(ch);
    return !(chinese || /[0-9]/.test(ch) || SPEAK_PUNCTUATIONS.includes(ch));
  });
  if (bad.length) {
    return {
      text: "",
      error: `只支持中文汉字、中文标点与数字（不支持：${Array.from(new Set(bad)).slice(0, 6).join("")}）`
    };
  }
  const chinese = (compact.match(/[\u4e00-\u9fff]/g) ?? []).length;
  if (chinese > SPEAK_MAX_CHARS) {
    return { text: "", error: `中文汉字不能超过 ${SPEAK_MAX_CHARS} 个（当前 ${chinese} 个）` };
  }
  if (compact.length > SPEAK_MAX_LEN) {
    return { text: "", error: `总长度不能超过 ${SPEAK_MAX_LEN} 个字符` };
  }
  return { text: compact, error: "" };
}
