# Renderer Process

React application running in Chromium.

## Structure
- `App.tsx` - Root layout
- `main.tsx` - React entry point
- `index.css` - Global styles with Tailwind
- `components/` - UI components by feature
- `store/` - Zustand state (slices pattern)
- `hooks/` - Custom React hooks
- `utils/` - Renderer utilities
- `types/` - Renderer type definitions
- `constants/` - CSS variables (`cssVariables.ts`), layout constants (`layout.ts`), team colors (`teamColors.ts`)
- `contexts/` - React contexts (`TabUIContext.tsx`, `useTabUIContext.ts`)

## Component Organization
```
components/
├── chat/           # Chat display, message items, viewers, context panel
├── common/         # Shared components (badges, dropdowns, token display)
├── dashboard/      # Dashboard views
├── layout/         # Layout components (headers, shells)
├── notifications/  # Notification panels and badges
├── search/         # Search UI and results
├── settings/       # Settings UI
└── sidebar/        # Sidebar navigation
```

## Types (`types/`)
- `data.ts` - Core data types (ParsedMessage, SemanticStep, SessionMetrics)
- `groups.ts` - Chat groups (UserGroup, AIGroup, SystemGroup, AIGroupDisplayItem union)
- `contextInjection.ts` - Context tracking (ContextInjection union, ContextStats, ContextPhaseInfo)
- `claudeMd.ts` - CLAUDE.md injection types
- `panes.ts` - Pane layout types
- `tabs.ts` - Tab management types
- `notifications.ts` - Notification types
- `api.ts` - API types

## Utils (`utils/`)
- `contextTracker.ts` - Visible context tracking (computeContextStats, processSessionContextWithPhases)
- `claudeMdTracker.ts` - CLAUDE.md injection detection
- `aiGroupEnhancer.ts` - AI group enrichment (linkToolCallsToResults, buildDisplayItems)
- `aiGroupHelpers.ts` - AI group utility functions
- `displayItemBuilder.ts` - Display item construction
- `displaySummary.ts` - Display summary generation
- `formatters.ts` - Display formatting
- `groupTransformer.ts` - Chat item grouping
- `lastOutputDetector.ts` - Last output detection
- `modelExtractor.ts` - Model name extraction
- `pathDisplay.ts` - Path display formatting
- `pathUtils.ts` - Path utility functions
- `slashCommandExtractor.ts` - Slash command extraction
- `stringUtils.ts` - String utility functions
- `toolLinkingEngine.ts` - Tool call/result linking
- `toolRendering/` - Tool rendering helpers
  - `toolContentChecks.ts` - Tool content validation
  - `toolSummaryHelpers.ts` - Tool summary formatting
  - `toolTokens.ts` - Tool token utilities

## Hooks
- `useAutoScrollBottom` - Auto-scroll chat to bottom
- `useKeyboardShortcuts` - Keyboard shortcuts
- `useTabNavigationController` - Turn navigation with highlighting
- `useTabUI` - Per-tab UI state access
- `useTheme` - Dark/light theme toggle
- `useVisibleAIGroup` - Viewport-aware AI group tracking
- `useZoomFactor` - Zoom level management
- `navigation/utils.ts` - Navigation utility functions

## Contexts
- `contexts/TabUIContext.tsx` - Per-tab UI state isolation
- `contexts/useTabUIContext.ts` - Context consumer hook

## State Management
Zustand store with slices pattern:
- Each domain has data, selectedId, loading, error
- Actions grouped by domain
- Selectors for derived state

## Virtual Scrolling
Use `@tanstack/react-virtual` for large lists (sessions, messages).
