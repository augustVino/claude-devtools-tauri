# Shared

Cross-process code used by main and renderer.

## What Goes Here
- Types shared between processes
- Pure utility functions (no Node/DOM APIs)
- Constants used across processes

## What Doesn't Go Here
- Node.js APIs → main/
- DOM/React APIs → renderer/
- Process-specific logic

## Structure
- `types/` - Shared type definitions (`api.ts`, `notifications.ts`, `visualization.ts`)
- `utils/` - Pure utility functions
  - `tokenFormatting.ts` - Token formatting and estimation (`estimateTokens`, `formatTokensCompact`)
  - `modelParser.ts` - Model name/family parsing
  - `teammateMessageParser.ts` - `<teammate-message>` XML parsing
  - `markdownTextSearch.ts` - Markdown-aware text search
  - `contentSanitizer.ts` - Content sanitization
  - `errorHandling.ts` - Error helpers
  - `logger.ts` - Logging utility
- `constants/` - Shared constants
  - `cache.ts` - Cache configuration
  - `trafficLights.ts` - macOS traffic light constants
  - `triggerColors.ts` - Trigger color palette
  - `window.ts` - Window configuration

## Import
```typescript
import { SomeType } from '@shared/types';
import { estimateTokens } from '@shared/utils/tokenFormatting';
```
