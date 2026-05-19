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

export function WelcomeHero() {
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

  const values = tip.buildValues?.({ shortcuts, isMac, kbd }) ?? {}
  const tipNode = t.rich(
    `tips.${tip.key}` as Parameters<typeof t.rich>[0],
    values as Parameters<typeof t.rich>[1]
  )

  return (
    <div className="flex w-full flex-col items-center gap-4">
      <div className="relative flex h-28 w-28 items-center justify-center">
        <div
          aria-hidden
          className="absolute inset-0 rounded-full bg-primary/30 blur-3xl"
        />
        <div
          aria-hidden
          className="absolute h-20 w-20 rounded-[28%] bg-primary/50 blur-xl"
        />
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img
          src="/icon.svg"
          alt=""
          className="relative h-16 w-16 rounded-[22%] shadow-2xl shadow-primary/40 ring-1 ring-foreground/10"
          draggable={false}
        />
      </div>

      <div className="flex flex-col items-center gap-2">
        <h1 className="text-3xl font-semibold tracking-tight">{t("title")}</h1>
        <div className="flex max-w-md items-start gap-2 text-sm text-muted-foreground">
          <span className="flex h-[1.375em] shrink-0 items-center">
            <Lightbulb aria-hidden className="h-4 w-4 text-muted-foreground" />
          </span>
          <p className="leading-snug">{tipNode}</p>
        </div>
      </div>
    </div>
  )
}
