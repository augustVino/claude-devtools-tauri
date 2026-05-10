# Shared

跨进程共享的代码。

## 放入条件

- 多进程共用的类型定义
- 纯工具函数（无 Node/DOM API 依赖）
- 跨进程常量

## 不要放入

- Node.js API → `main/`
- DOM/React API → `src/` 渲染层
- 进程特定逻辑

## 目录

| 目录 | 内容 |
|------|------|
| `types/` | 共享类型（api.ts、notifications.ts、visualization.ts） |
| `utils/` | 纯工具函数（tokenFormatting、modelParser、teammateMessageParser、markdownTextSearch、contentSanitizer、sessionIdValidator、errorHandling、logger） |
| `constants/` | 共享常量（cache、trafficLights、triggerColors、window） |

## 导入方式

```typescript
import type { SomeType } from '@shared/types/api';
import { estimateTokens } from '@shared/utils/tokenFormatting';
```
