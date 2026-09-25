/** 界面之间共用的两个小东西：一个操作类型，和卡片号的显示方式。 */

/**
 * 跑一次设备操作。
 *
 * `busy` 是这次操作的中文说明，界面靠它显示进度并禁用按钮；失败时后端已经给出中文
 * 句子，这里只负责把它显示出来，外加把列表刷新回来。
 */
export type Run = (label: string, work: () => Promise<void>) => Promise<void>;

/** 卡片号很长，列表里只显示头尾。 */
export function shortHash(hash: string, keep = 8): string {
  if (hash.length <= keep * 2 + 1) return hash;
  return `${hash.slice(0, keep)}…${hash.slice(-keep)}`;
}
