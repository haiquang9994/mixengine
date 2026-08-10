# Frontend standards (`apps/desktop`)

Stack: Tauri v2 · React 18 · TypeScript (strict) · Vite · TanStack Query · Tailwind + a small local
component layer.

## Boundaries

```
src/
  api/          generated types (ts-rs) + one client module per RPC namespace + the SSE subscriber
  state/        query keys, query/mutation hooks, event→invalidation mapping
  features/     one folder per screen: components, hooks, tests, colocated
  ui/           design primitives (Button, Dialog, Table, StatusDot…) — imports nothing from features
  lib/          pure helpers (formatting bytes, durations, validation)
```

Rules:

- **Only `src/api/` constructs RPC payloads.** A component that builds a JSON-RPC body is a bug.
- **Only `src/state/` calls `src/api/`.** Components use hooks.
- `ui/` never imports from `features/` or `state/`. Enforced by an ESLint boundary rule.
- The Tauri Rust side is a transport proxy only; no logic there (see [../features/gui.md](../features/gui.md)).

## Types

- `strict: true`, `noUncheckedIndexedAccess: true`, `exactOptionalPropertyTypes: true`.
- **`any` is banned**; `unknown` + a narrowing function instead. Wire types are generated from
  `mixengine-proto` — never hand-write a type that mirrors a Rust struct, regenerate it
  (`cargo run -p mixengine-proto --bin export-bindings`) and commit the output.
- Discriminated unions for states (`ServiceState`), and exhaustive `switch` with a `never` fallback
  so adding a Rust variant breaks the TypeScript build. That is the point.

## Server state

- TanStack Query owns all server data. **No Redux/Zustand copy of it.** Local UI state (dialog open,
  form draft) uses `useState`/`useReducer`.
- Query keys are centralised in `state/keys.ts`; every mutation declares what it invalidates.
- One SSE subscriber maps `DaemonEvent` → invalidations. Components never subscribe individually.
- On reconnect: invalidate everything (the daemon may have changed while we were away).

## Components

- Function components, named exports, one component per file when it exceeds ~40 lines.
- Props are explicit interfaces; no prop spreading through more than one level.
- Loading and error states are required for every async view — a component that renders `undefined`
  while loading does not pass review. Use skeletons matching the final layout, not spinners that
  cause layout shift.
- No `useEffect` for data fetching; that is what Query is for. `useEffect` is for subscriptions and
  imperative DOM only.

## Styling

- Tailwind utilities in components; tokens (colour, spacing, radius) defined once in the theme.
- Light and dark both mandatory; never hardcode a colour outside the token set.
- Respect `prefers-reduced-motion`; animations are ≤ 200 ms and never block interaction.

## Accessibility

- Every interactive element is keyboard reachable with a visible focus ring.
- Dialogs trap focus and restore it; lists have proper roles; icons have labels.
- State is never conveyed by colour alone — pair the dot with text.

## i18n

- English and Vietnamese from the start. All user-facing strings go through the i18n layer; no
  literals in JSX. Keys are namespaced by feature. Pluralisation via the ICU form.

## Testing

Vitest + React Testing Library for hooks and components against a mocked API client; Playwright for
the few end-to-end flows that matter (create site → open it). Details in [testing.md](testing.md).
