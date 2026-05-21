"use client"

import { useState, type ReactNode } from "react"
import { useTranslations } from "next-intl"
import { Lightbulb } from "lucide-react"
import { useShortcutSettings } from "@/hooks/use-shortcut-settings"
import { useIsMac } from "@/hooks/use-is-mac"
import {
  formatShortcutLabel,
  type ShortcutSettings,
} from "@/lib/keyboard-shortcuts"

type TipKey =
  | "tileTabs"
  | "pinTab"
  | "shortcutsNewSearch"
  | "slashAtMention"
  | "pasteDropFiles"
  | "queueMessage"
  | "draftAutoSave"
  | "forkSend"
  | "exportConversation"
  | "chatChannels"
  | "shortcutsAuxPanel"
  | "shortcutsTerminalSidebar"
  | "customShortcuts"
  | "webService"
  | "fusionMode"
  | "quickMessages"
  | "experts"

interface TipDef {
  key: TipKey
  buildValues?: (ctx: {
    shortcuts: ShortcutSettings
    isMac: boolean
    kbd: (chunks: ReactNode) => ReactNode
  }) => Record<string, ReactNode | ((chunks: ReactNode) => ReactNode) | string>
}

const TIPS: TipDef[] = [
  { key: "tileTabs" },
  { key: "pinTab" },
  {
    key: "shortcutsNewSearch",
    buildValues: ({ shortcuts, isMac, kbd }) => ({
      shortcut: kbd,
      newConversation: formatShortcutLabel(shortcuts.new_conversation, isMac),
      searchConversations: formatShortcutLabel(shortcuts.toggle_search, isMac),
    }),
  },
  { key: "slashAtMention" },
  { key: "pasteDropFiles" },
  { key: "queueMessage" },
  { key: "draftAutoSave" },
  { key: "forkSend" },
  { key: "exportConversation" },
  { key: "chatChannels" },
  {
    key: "shortcutsAuxPanel",
    buildValues: ({ shortcuts, isMac, kbd }) => ({
      shortcut: kbd,
      toggleAuxPanel: formatShortcutLabel(shortcuts.toggle_aux_panel, isMac),
    }),
  },
  {
    key: "shortcutsTerminalSidebar",
    buildValues: ({ shortcuts, isMac, kbd }) => ({
      shortcut: kbd,
      toggleTerminal: formatShortcutLabel(shortcuts.toggle_terminal, isMac),
      toggleSidebar: formatShortcutLabel(shortcuts.toggle_sidebar, isMac),
    }),
  },
  { key: "customShortcuts" },
  { key: "webService" },
  { key: "fusionMode" },
  { key: "quickMessages" },
  { key: "experts" },
]

const highlightTitle = (chunks: ReactNode) => (
  <span className="bg-gradient-to-br from-primary via-primary/85 to-chart-3 bg-clip-text text-transparent">
    {chunks}
  </span>
)

const highlightTip = (chunks: ReactNode) => (
  <span className="font-medium text-primary">{chunks}</span>
)

export function WelcomeHero() {
  const t = useTranslations("Folder.chat.welcomePanel")

  return (
    <h1 className="text-center text-3xl font-semibold tracking-tight text-foreground sm:text-4xl">
      {t.rich("greeting", { highlight: highlightTitle })}
    </h1>
  )
}

export function WelcomeBackdrop() {
  return (
    <div
      aria-hidden
      className="pointer-events-none absolute inset-0 -z-10 overflow-hidden"
    >
      <div className="absolute -top-48 -left-48 h-[36rem] w-[36rem] rounded-full bg-gradient-to-br from-primary/8 via-primary/3 to-transparent blur-[120px]" />
      <div className="absolute -top-56 -right-48 h-[32rem] w-[32rem] rounded-full bg-gradient-to-bl from-chart-3/8 via-chart-3/3 to-transparent blur-[120px]" />
      <div className="absolute -bottom-56 -left-48 h-[32rem] w-[32rem] rounded-full bg-gradient-to-tr from-chart-3/6 via-chart-3/2 to-transparent blur-[120px]" />
      <div className="absolute -bottom-48 -right-48 h-[36rem] w-[36rem] rounded-full bg-gradient-to-tl from-primary/7 via-primary/3 to-transparent blur-[120px]" />
    </div>
  )
}

export function WelcomeTip() {
  const t = useTranslations("Folder.chat.welcomePanel")
  const { shortcuts } = useShortcutSettings()
  const isMac = useIsMac()

  const [tipIndex] = useState(() => Math.floor(Math.random() * TIPS.length))
  const tip = TIPS[tipIndex]

  const kbd = (chunks: ReactNode) => (
    <kbd className="mx-0.5 inline-flex items-center rounded border border-border bg-muted px-1.5 py-0.5 font-mono text-[10.5px] font-medium text-foreground/80">
      {chunks}
    </kbd>
  )

  const values = {
    ...(tip.buildValues?.({ shortcuts, isMac, kbd }) ?? {}),
    highlight: highlightTip,
  }
  const tipNode = t.rich(
    `tips.${tip.key}` as Parameters<typeof t.rich>[0],
    values as Parameters<typeof t.rich>[1]
  )

  return (
    <div className="flex max-w-full justify-center">
      <div className="flex max-w-full items-start gap-2 rounded-full border border-border/40 bg-gradient-to-r from-primary/5 via-muted/40 to-chart-3/5 px-4 py-1.5 text-center text-xs text-muted-foreground/90 backdrop-blur-sm">
        <span className="flex h-[1.375em] shrink-0 items-center">
          <Lightbulb aria-hidden className="h-3.5 w-3.5 text-primary" />
        </span>
        <p className="min-w-0 leading-snug">{tipNode}</p>
      </div>
    </div>
  )
}
