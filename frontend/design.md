# Frontend Design System and Product Guidelines

> **Status:** Design specification · **Product:** School Course Application System · **Scope:** Student and administrator web application  
> **Implementation context:** Next.js 16, React 19, TypeScript, Tailwind CSS 4 (see `frontend/package.json`).  
> This is a design contract, not a claim that the described components already exist.

## 1. Product intent

School is a task-oriented academic application. Students should be able to find courses, understand requirements, submit an application, and track decisions. Administrators should be able to review and resolve applications quickly and accurately.

The interface should feel like **credible university software**: restrained, legible, calm, reliable, and information-dense where needed. Prioritize usability and trust over spectacle. Avoid a generic AI-generated SaaS aesthetic.

**Design principles**
1. **Clarity over decoration.** Every visual element should help orientation, comprehension, or action.
2. **A recognizable identity, not a template.** Use a disciplined academic visual language and consistent components.
3. **Content first.** Real course codes, prerequisites, deadlines, statuses, and administrative context are the focal points.
4. **Progressive disclosure.** Show the essential information first; expose detail without overwhelming the user.
5. **Accessible by default.** Keyboard, screen reader, reduced-motion, and mobile usage are first-class.
6. **Predictable interactions.** Forms, confirmations, errors, and navigation behave consistently across the application.

## 2. What to avoid (anti-patterns)

Do **not** default to:
- Huge gradient hero sections, glowing orbs, floating blobs, decorative particles, or glassmorphism.
- Purple-to-blue gradients everywhere, neon accents, and arbitrary accent colours.
- Oversized headings with vague slogans such as “Unlock your academic journey”.
- A dashboard filled with unrelated KPI cards, charts, and fake statistics.
- Rounded-everything layouts, heavy shadows, or cards nested inside more cards.
- Emoji used as navigation icons; mismatched icon families; excessive iconography.
- Generic stock photos of students, fake testimonials, invented university logos, or fabricated data.
- Animations on every scroll, hover, and page transition.
- Inconsistent spacing, random border radii, or many slightly different button styles.
- Blank screens with no loading, empty, error, or permission states.

Do **not** interpret “minimal” as “empty”. The application should expose meaningful details with excellent hierarchy.

## 3. Visual direction

**Character:** institutional, contemporary, practical, quietly polished. Think modern registrar portal rather than startup landing page.

### Colour tokens

Use semantic CSS variables and Tailwind utilities; avoid arbitrary hex values in components. The following are starting values, not an institutional brand claim.

| Token | Light value | Usage |
| --- | --- | --- |
| `--background` | `#F8F9FA` | App background |
| `--surface` | `#FFFFFF` | Forms, tables, panels |
| `--surface-muted` | `#F1F4F6` | Subtle sections, hover |
| `--foreground` | `#17212B` | Main text |
| `--foreground-muted` | `#586675` | Secondary text |
| `--border` | `#DCE2E7` | Dividers and controls |
| `--primary` | `#174A67` | Primary actions, active navigation |
| `--primary-hover` | `#103B53` | Primary hover |
| `--focus` | `#146B9A` | Visible focus ring |
| `--success` | `#176449` | Accepted / success |
| `--warning` | `#895800` | Pending / caution |
| `--danger` | `#A12B36` | Rejected / destructive |

Ensure actual foreground/background pairs meet WCAG contrast requirements; do not assume a token alone is sufficient. Use status text and icons alongside colour. Dark mode is optional; if implemented, use a complete semantic token set rather than CSS inversion.

### Typography

- Prefer a single well-crafted sans-serif family (e.g. Inter, Source Sans 3, or a dependable system stack). Avoid mixing three or four fonts.
- Use a restrained type scale: 12 / 14 / 16 / 18 / 24 / 32 px. Body copy: 14–16 px with comfortable line-height.
- Page title: 24–32 px, semibold; section title: 18–20 px; labels: 13–14 px.
- Course codes, dates, and identifiers should remain scannable. Use tabular numerals where alignment matters.
- Avoid all-caps headings and excessive letter spacing; reserve uppercase for short metadata labels if needed.
- Do not force every page heading into a large marketing-style hero.

### Spacing and shape

- Use a 4 px spacing base: 4, 8, 12, 16, 24, 32, 48, 64.
- Default control height: approximately 40–44 px; touch targets at least 44×44 px where practical.
- Radius: 6–8 px for inputs and buttons, 8–12 px for panels. Avoid large pill shapes everywhere.
- Prefer borders and whitespace over shadows; if necessary, use one subtle elevation level.
- Content width: about 1120–1280 px for application screens; narrower (600–760 px) for forms and reading.
- Left-align primary content. Avoid centring long explanatory copy.

## 4. Information architecture

### Student navigation
- **Courses:** browse and inspect available offerings.
- **My Applications:** review submitted applications and their statuses.
- **Account:** profile, linked sign-in methods (when supported), security, and account deletion.

### Administrator navigation
- **Applications:** review queue with filters and decision tools.
- **Courses:** manage course offerings.
- **Account:** profile and security settings.

Only display navigation that the authenticated role can use. Authorization must also be enforced by the backend.

### Layout patterns
- Desktop: compact top bar with product name, primary navigation, and account menu; optional left sidebar for complex admin workflows.
- Mobile: accessible collapsible navigation; never hide essential actions behind hover-only menus.
- Every page: descriptive heading, brief context when necessary, and a clear primary action.
- Use breadcrumbs only for nested views where they improve orientation.
- Prefer tables for administrative review queues, and list rows or restrained cards for student course browsing.

## 5. Page-level specifications

### 5.1 Course catalogue
- Page title: **Courses**. Include academic term when relevant.
- Search by course name/code; filters only for fields actually supported by the backend.
- Each result shows course code, title, short description, application deadline, and availability when authoritative.
- Use one consistent “View details” affordance. Do not show an “Apply” button if eligibility/status is unknown without clearly communicating that.
- Include useful empty search results, loading skeletons, and a recoverable error state.
- Use real seed/test content labelled as such in development; no invented live statistics.

### 5.2 Course detail
- Clear title with course code, term, description, prerequisites, deadline, and capacity/availability if supported.
- A distinct application action and concise explanation of what happens next.
- Disable or replace the action with a reason for closed applications, already-applied courses, or unavailable offerings.
- Separate confirmed requirements from advisory information.

### 5.3 Application submission
- Keep the workflow short. Show a review/confirmation step only when it prevents meaningful errors.
- Display course, term, key requirements, and an explicit submission action.
- Prevent accidental double submissions and show server-side validation errors inline.
- On success, show confirmation and a direct link to **My Applications**.
- Never claim an application was submitted until the server confirms it.

### 5.4 My Applications
- Show course, submitted date, current status, and next steps.
- Status vocabulary: **Pending**, **Accepted**, **Rejected**. Use consistent semantic badges with text.
- Do not equate “Accepted” with “Enrolled” unless enrolment is actually implemented.
- Provide an intentional empty state with a link to browse courses.

### 5.5 Administrator review queue
- Table columns: student, course, submitted date, status, and review action.
- Filtering/sorting should reflect server-supported semantics; preserve query state in the URL where useful.
- Row selection opens a review detail view with application context and decision history.
- Acceptance/rejection requires clear confirmation when consequential, and records who decided and when.
- Avoid colour-only distinctions and misleading optimistic success states.
- For large datasets, use server pagination and clear total/result counts if available.

### 5.6 Authentication and account
- Compact, readable login/signup screens with clear labels, password visibility toggle, and useful validation.
- Google and Microsoft sign-in buttons should use accurate provider names/approved marks; never fake provider branding.
- Explain email verification, password reset, session expiry, and sign-out states.
- Account settings include editable profile information, security actions, and a separate destructive account deletion section.
- Account deletion requires explicit consequences and recent reauthentication; do not use a misleading one-click delete.

## 6. Component design rules

Build a small, deliberate component library rather than a sprawling set of variants.

**Primitives:** Button, TextField, Textarea, Select, Checkbox, Radio, Badge, Alert, Dialog, DropdownMenu, Tabs, Table, Pagination, Skeleton, EmptyState, PageHeader, FormField.

**Conventions**
- Buttons: primary, secondary, tertiary/ghost, destructive; small/default sizes only unless justified.
- One primary action per local context. Secondary actions must not visually compete.
- Inputs have persistent labels, optional help text, visible focus, and inline error messages.
- Dialogs have a clear title, keyboard focus management, Escape handling where safe, and a sensible return-focus target.
- Status badges must use text, not just colour.
- Tables need real headers, appropriate row actions, and a usable narrow-screen alternative.
- Icons should come from one consistent library and be used only when they add meaning.
- Avoid bespoke components when a semantic HTML element or existing primitive suffices.

## 7. Interaction and state conventions

Every data-driven screen must explicitly handle:
1. **Loading:** stable layout with skeletons or restrained progress indicators.
2. **Empty:** distinguish no data from no search results.
3. **Error:** explain what failed and offer retry where meaningful.
4. **Success:** confirm only after server acknowledgement.
5. **Forbidden/unauthenticated:** explain access restrictions and provide a recovery path.
6. **Stale data:** refresh after mutations and show authoritative server state.

- Prefer native navigation and links for navigation, buttons for actions.
- Keep transitions subtle (roughly 120–180 ms) and respect `prefers-reduced-motion`.
- Never animate layout just to make the product feel “premium”.
- Avoid toast-only error reporting for forms; errors must remain visible in context.
- Confirm destructive actions; do not over-confirm routine actions.

## 8. Responsive behaviour

- **Small screens (< 640 px):** single column, full-width form controls, readable list alternatives for wide tables.
- **Medium screens (640–1023 px):** adaptive spacing and condensed navigation.
- **Large screens (≥ 1024 px):** stable content max-width and two-column layouts only where they improve comprehension.
- Avoid horizontal scrolling of the whole page; table regions may scroll intentionally with visible affordances.
- Test at 320 px width, common mobile widths, tablet, and desktop. Do not rely on one screenshot size.

## 9. Accessibility and content standards

- Target WCAG 2.2 AA for applicable UI.
- Use semantic headings, landmarks, labels, table headers, and proper form associations.
- Ensure keyboard-only navigation, visible focus, and accessible error summaries.
- Verify contrast (4.5:1 for normal text, 3:1 for large text and relevant UI boundaries).
- Never use placeholders as labels.
- Announce important asynchronous status changes accessibly.
- Write plain, specific microcopy: “Submit application”, “Application submitted”, “Application deadline has passed”.
- Use Canadian English consistently (e.g. **enrolment**, **centre**, **cancelled**); use localized date/time formats and include time zone when deadlines matter.
- Do not fabricate acceptance rates, course availability, student counts, or deadlines.

## 10. Next.js and frontend engineering

- Prefer Next.js App Router and server components for non-interactive data presentation; use client components only where interactivity requires them.
- Keep server-only credentials and privileged backend calls on the server.
- Treat the Rust API as authoritative for authorization, application state, and validation.
- Define typed API request/response contracts and centralize HTTP error translation.
- Never put refresh tokens in localStorage; coordinate secure HttpOnly cookie behaviour and CSRF protection with backend design.
- Use feature-oriented modules for courses, applications, auth, and admin rather than a giant `components/` folder.
- Keep component styles token-driven; no ad hoc arbitrary values unless documented.
- Use Tailwind 4 theme tokens/CSS variables; avoid a separate, conflicting style system.
- Prefer server-rendered initial states when appropriate, with explicit client-side mutation and refresh patterns.

**Suggested structure (adapt to existing code):**

```text
frontend/
├── app/
│   ├── (auth)/
│   ├── (student)/
│   ├── (admin)/
│   └── globals.css
├── components/
│   ├── ui/
│   └── layout/
├── features/
│   ├── auth/
│   ├── courses/
│   ├── applications/
│   └── admin/
├── lib/
│   ├── api/
│   ├── auth/
│   └── utils/
└── tests/
```

## 11. Design review checklist

Before merging a new page or major UI change:

- [ ] Does it solve a real student/admin task without decorative clutter?
- [ ] Are hierarchy, spacing, typography, colours, and components consistent with this document?
- [ ] Are real course/application fields presented accurately without fabricated data?
- [ ] Is there one clear primary action and understandable navigation?
- [ ] Are loading, empty, error, success, and permission states covered?
- [ ] Does it work on keyboard, screen reader, mobile, and reduced-motion settings?
- [ ] Are destructive actions safe and status changes backed by the API?
- [ ] Are there screenshots for mobile and desktop, and were they reviewed?
- [ ] Does the result feel like a cohesive academic product rather than a generic SaaS template?

## 12. Design decisions to revisit

- Confirm institutional branding and whether any school-specific colours, logos, or typography are authorized.
- Confirm supported course filters and prerequisite information before designing controls.
- Confirm whether acceptance is a final decision or a precursor to enrolment.
- Confirm backend auth transport and CSRF strategy before finalizing the frontend session architecture.
- Decide whether admin users require a persistent sidebar as their workflow grows.

**North star:** An interface should look intentionally designed because its information, typography, spacing, and interactions are coherent—not because it uses more effects.
