/**
 * 并发限制批处理器 — Worker pool 模式。
 * 保证同时最多只有 concurrency 个异步操作在飞行中。
 * 用于替代无限制的 Promise.all，防止 SSH 模式下 SFTP 连接耗尽。
 */
export async function batchAsync<T, R>(
  items: T[],
  fn: (item: T) => Promise<R>,
  concurrency = 5,
): Promise<R[]> {
  const results = new Array<R>(items.length);
  let nextIndex = 0;

  async function worker(): Promise<void> {
    while (nextIndex < items.length) {
      const index = nextIndex++;
      results[index] = await fn(items[index]);
    }
  }

  await Promise.all(
    Array.from({ length: Math.min(concurrency, items.length) }, () => worker()),
  );
  return results;
}
