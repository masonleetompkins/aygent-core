// DASHBOARD PRESETS (M5) — global, zero-cost starting points.
//
// Every preset ships with ONLY free data sources (static + binding). Picking a
// preset must never cost money or surprise the user with a model call — that's
// the safety rule applied to onboarding.
//
// These double as few-shot examples: the agent reads them when asked to build
// something custom, so quality doesn't depend on the model guessing taste.
import type { ModuleSpec } from "./Modules";

export interface Preset {
  id: string;
  name: string;
  blurb: string;
  modules: Omit<ModuleSpec, "id">[];
}

export const PRESETS: Preset[] = [
  {
    id: "command-center",
    name: "Command Center",
    blurb: "Agent status, schedules, recent runs, and quick prompts.",
    modules: [
      {
        kind: "stat", title: "Modules", layout: { x: 0, y: 0, w: 3, h: 3 },
        source: { kind: "binding", path: "dashboard.modules" }, props: {},
      },
      {
        kind: "status", title: "Connections", layout: { x: 3, y: 0, w: 5, h: 3 },
        source: { kind: "binding", path: "connections" }, props: {},
      },
      {
        kind: "actions", title: "Quick Prompts", layout: { x: 8, y: 0, w: 4, h: 3 },
        source: { kind: "static", data: null }, props: {},
        actions: [
          { label: "Daily brief", action: { kind: "run_prompt", prompt: "Give me a short brief on what I should focus on today.", open_chat: true } },
          { label: "What changed?", action: { kind: "run_prompt", prompt: "Summarize what changed in my work since yesterday.", open_chat: true } },
        ],
      },
      {
        kind: "list", title: "Schedules", layout: { x: 0, y: 3, w: 6, h: 6 },
        source: { kind: "binding", path: "schedules" }, props: {},
      },
      {
        kind: "timeline", title: "Recent Runs", layout: { x: 6, y: 3, w: 6, h: 6 },
        source: { kind: "binding", path: "schedule_runs" }, props: {},
      },
    ],
  },
  {
    id: "build-ops",
    name: "Build / Ops",
    blurb: "Git state, build status, and a one-click build. Needs Allow Shell Access.",
    modules: [
      {
        kind: "list", title: "Recent Commits", layout: { x: 0, y: 0, w: 7, h: 6 },
        // Proposed, NOT approved — renders pending until the user OKs the exact
        // command. This is the approval gate doing its job on a preset.
        source: { kind: "exec", cmd: "git log --oneline -8" }, props: {},
      },
      {
        kind: "list", title: "Working Tree", layout: { x: 7, y: 0, w: 5, h: 6 },
        source: { kind: "exec", cmd: "git status --short" }, props: {},
      },
      {
        kind: "actions", title: "Actions", layout: { x: 0, y: 6, w: 12, h: 3 },
        source: { kind: "static", data: null }, props: {},
        actions: [
          { label: "Explain last commit", action: { kind: "run_prompt", prompt: "Read the last commit and explain what changed and why.", open_chat: true } },
          { label: "Open Scheduler", action: { kind: "navigate", screen: "scheduler" } },
        ],
      },
    ],
  },
  {
    id: "content-studio",
    name: "Content Studio",
    blurb: "Pipeline, idea inbox, and a draft button wired to your Daily note.",
    modules: [
      {
        kind: "stat", title: "Published", layout: { x: 0, y: 0, w: 3, h: 3 },
        source: { kind: "static", data: { value: 0, label: "this week" } }, props: {},
      },
      {
        kind: "progress", title: "Weekly Goal", layout: { x: 3, y: 0, w: 5, h: 3 },
        source: { kind: "static", data: { value: 0, goal: 3, label: "posts" } }, props: {},
      },
      {
        kind: "actions", title: "Write", layout: { x: 8, y: 0, w: 4, h: 3 },
        source: { kind: "static", data: null }, props: {},
        actions: [
          { label: "Draft from today's Daily", action: { kind: "run_prompt", prompt: "Read today's Daily note and draft one piece of content from a concrete detail in it — a number, a name, or a scene. No generic advice.", open_chat: true } },
        ],
      },
      {
        kind: "table", title: "Pipeline", layout: { x: 0, y: 3, w: 7, h: 6 },
        source: { kind: "static", data: [
          { idea: "—", stage: "draft", due: "—" },
        ] },
        props: { columns: ["idea", "stage", "due"] },
      },
      {
        kind: "markdown", title: "Idea Inbox", layout: { x: 7, y: 3, w: 5, h: 6 },
        source: { kind: "static", data: { text: "Drop raw ideas here.\n\n- \n- \n- " } }, props: {},
      },
    ],
  },
  {
    id: "research-desk",
    name: "Research Desk",
    blurb: "Reading queue, sources, and notes.",
    modules: [
      {
        kind: "markdown", title: "Current Question", layout: { x: 0, y: 0, w: 7, h: 4 },
        source: { kind: "static", data: { text: "**What am I actually trying to find out?**\n\n_Write it in one sentence._" } }, props: {},
      },
      {
        kind: "actions", title: "Research", layout: { x: 7, y: 0, w: 5, h: 4 },
        source: { kind: "static", data: null }, props: {},
        actions: [
          { label: "Summarize my sources", action: { kind: "run_prompt", prompt: "Read my recent research notes and summarize what I've learned, flagging contradictions.", open_chat: true } },
        ],
      },
      {
        kind: "list", title: "Reading Queue", layout: { x: 0, y: 4, w: 6, h: 6 },
        source: { kind: "static", data: [] }, props: {},
      },
      {
        kind: "markdown", title: "Notes", layout: { x: 6, y: 4, w: 6, h: 6 },
        source: { kind: "static", data: { text: "_Notes go here._" } }, props: {},
      },
    ],
  },
  {
    id: "life",
    name: "Life",
    blurb: "Habits, a couple of numbers, and a journal prompt.",
    modules: [
      {
        kind: "progress", title: "Habit Streak", layout: { x: 0, y: 0, w: 4, h: 3 },
        source: { kind: "static", data: { value: 0, goal: 30, label: "days" } }, props: {},
      },
      {
        kind: "stat", title: "Focus Hours", layout: { x: 4, y: 0, w: 4, h: 3 },
        source: { kind: "static", data: { value: 0, label: "this week" } }, props: {},
      },
      {
        kind: "actions", title: "Reflect", layout: { x: 8, y: 0, w: 4, h: 3 },
        source: { kind: "static", data: null }, props: {},
        actions: [
          { label: "Journal prompt", action: { kind: "run_prompt", prompt: "Ask me one specific, non-generic reflection question based on what you know about my week.", open_chat: true } },
        ],
      },
      {
        kind: "chart", title: "Trend", layout: { x: 0, y: 3, w: 12, h: 5 },
        source: { kind: "static", data: { points: [1, 2, 2, 3, 5, 4, 6] } },
        props: { variant: "line" },
      },
    ],
  },
];
