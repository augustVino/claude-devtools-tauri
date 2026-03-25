# Components

UI components organized by feature domain.

## Structure
```
components/
├── chat/                    # Session message display
│   ├── items/               # Individual message/tool items
│   │   ├── linkedTool/      # Tool call/result display helpers
│   │   ├── BaseItem         # Base item wrapper
│   │   ├── baseItemHelpers  # Base item utility functions
│   │   ├── ExecutionTrace   # Execution trace display
│   │   ├── LinkedToolItem   # Tool call with linked result
│   │   ├── MetricsPill      # Metrics badge display
│   │   ├── SlashItem        # Slash command display
│   │   ├── SubagentItem     # Subagent execution display
│   │   ├── TeammateMessageItem  # Team message cards
│   │   ├── ThinkingItem     # Extended thinking display
│   │   └── TextItem         # Text output display
│   ├── viewers/             # Content viewers (JSON, code, diff)
│   ├── SessionContextPanel/ # Visible context tracking panel
│   │   ├── components/      # Section wrappers (ClaudeMdFilesSection, ToolOutputsSection, UserMessagesSection, etc.)
│   │   ├── items/           # Per-injection item renderers (ClaudeMdItem, ToolOutputItem, UserMessageItem, etc.)
│   │   ├── DirectoryTree/   # CLAUDE.md directory navigation
│   │   ├── utils/           # Formatting helpers
│   │   ├── index.tsx        # Main panel component
│   │   └── types.ts         # SectionType constants, panel props
│   ├── AIChatGroup.tsx      # AI response group display
│   ├── ChatHistory.tsx      # Chat timeline container
│   ├── ChatHistoryEmptyState.tsx  # Empty state display
│   ├── ChatHistoryItem.tsx  # Individual history item
│   ├── ChatHistoryLoadingState.tsx # Loading state display
│   ├── CompactBoundary.tsx  # Compaction event boundary marker
│   ├── ContextBadge.tsx     # Per-turn context injection popover badge
│   ├── DisplayItemList.tsx  # Display item list rendering
│   ├── LastOutputDisplay.tsx # Last output display
│   ├── SystemChatGroup.tsx  # System message group display
│   ├── UserChatGroup.tsx    # User message display
│   ├── markdownComponents.tsx # Custom markdown renderers
│   └── searchHighlightUtils.ts # Search highlight utilities
├── common/                  # Shared UI primitives
│   ├── CopyButton           # Copy to clipboard button
│   ├── CopyablePath         # Clickable, copyable file path
│   ├── ErrorBoundary        # React error boundary
│   ├── OngoingIndicator     # Session in-progress indicator
│   ├── RepositoryDropdown   # Repository selector dropdown
│   ├── TokenUsageDisplay    # Token breakdown with context stats hover
│   └── WorktreeBadge        # Git worktree badge
├── dashboard/               # Overview and listing pages
├── layout/                  # App shell, sidebars, headers
├── notifications/           # Notification panels and badges
├── search/                  # Search UI and results
├── settings/                # Settings pages and controls
│   ├── components/          # Reusable setting controls (SettingRow, SettingsToggle, etc.)
│   ├── hooks/               # Settings-specific hooks
│   ├── sections/            # Setting sections (General, Notifications, Advanced)
│   └── NotificationTriggerSettings/  # Trigger config UI
│       ├── components/      # Trigger form components
│       ├── hooks/           # Trigger form hooks
│       └── utils/           # Trigger utilities
└── sidebar/                 # Project/session navigation
```

## Adding Components
1. Choose appropriate parent directory by feature
2. If used across features, place in `common/`
3. Use Tailwind with theme-aware CSS variables
4. Connect to store via `useStore()` hook if needed
5. Colocate related hooks/utils in same directory

## Component Guidelines
- One component per file, PascalCase naming
- Use functional components with hooks
- Prefer composition over prop drilling
- Use `TabUIContext` for per-tab UI state

## Virtual Scrolling
Use `@tanstack/react-virtual` for lists > 100 items:
- Session lists in sidebar
- Message lists in chat view
