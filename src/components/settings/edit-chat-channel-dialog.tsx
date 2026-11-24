"use client"

import { useCallback, useEffect, useId, useMemo, useState } from "react"
import { Loader2 } from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Switch } from "@/components/ui/switch"
import {
  updateChatChannel,
  saveChatChannelToken,
  getChatChannelHasToken,
} from "@/lib/api"
import type { ChatChannelInfo } from "@/lib/types"
import { toErrorMessage } from "@/lib/app-error"

interface EditChatChannelDialogProps {
  open: boolean
  channel: ChatChannelInfo
  onOpenChange: (open: boolean) => void
  onChannelUpdated: () => void
  onOpenWelinkAuth?: (channelId: number) => void
}

export function EditChatChannelDialog({
  open,
  channel,
  onOpenChange,
  onChannelUpdated,
  onOpenWelinkAuth,
}: EditChatChannelDialogProps) {
  const t = useTranslations("ChatChannelSettings")
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const config = useMemo(
    () => JSON.parse(channel.config_json || "{}"),
    [channel.config_json]
  )
  const [name, setName] = useState(channel.name)
  const [token, setToken] = useState("")
  const [chatId, setChatId] = useState(config.chat_id ?? "")
  const [appId, setAppId] = useState(config.app_id ?? "")
  const [baseUrl] = useState(config.base_url ?? "")
  const [welinkGatewayUrl, setWelinkGatewayUrl] = useState(
    config.gateway_url ?? ""
  )
  const [welinkAssistantName, setWelinkAssistantName] = useState(
    config.assistant_name ?? "codeagent"
  )
  const [welinkAk, setWelinkAk] = useState(config.ak ?? "")
  const [welinkSk, setWelinkSk] = useState("")
  const [topicMode, setTopicMode] = useState(Boolean(config.topic_mode))
  const [dailyReportEnabled, setDailyReportEnabled] = useState(
    channel.daily_report_enabled
  )
  const [dailyReportTime, setDailyReportTime] = useState(
    channel.daily_report_time || "18:00"
  )
  const [hasToken, setHasToken] = useState(false)
  const topicModeId = useId()

  useEffect(() => {
    if (open) {
      getChatChannelHasToken(channel.id)
        .then(setHasToken)
        .catch(() => {})
    }
  }, [open, channel.id])

  const submitUpdate = useCallback(
    async (launchWelinkAuth: boolean) => {
      if (!name.trim()) {
        setError(t("nameRequired"))
        return
      }
      if (channel.channel_type === "welink") {
        if (!welinkGatewayUrl.trim()) {
          setError(t("welinkGatewayUrlRequired"))
          return
        }
      } else if (channel.channel_type !== "weixin" && !chatId.trim()) {
        setError(t("chatIdRequired"))
        return
      }

      setLoading(true)
      setError(null)
      try {
        const configJson =
          channel.channel_type === "weixin"
            ? JSON.stringify({ base_url: baseUrl })
            : channel.channel_type === "lark"
              ? JSON.stringify({ app_id: appId, chat_id: chatId })
              : channel.channel_type === "welink"
                ? JSON.stringify({
                    ...config,
                    assistant_name: welinkAssistantName.trim() || "codeagent",
                    gateway_url: welinkGatewayUrl.trim(),
                    ak: welinkAk.trim(),
                  })
                : JSON.stringify({ chat_id: chatId, topic_mode: topicMode })

        await updateChatChannel({
          id: channel.id,
          name: name.trim(),
          configJson,
          dailyReportEnabled,
          dailyReportTime: dailyReportEnabled ? dailyReportTime : null,
        })

        if (channel.channel_type === "welink") {
          if (welinkSk.trim()) {
            await saveChatChannelToken(channel.id, welinkSk.trim())
          }
        } else if (token.trim()) {
          await saveChatChannelToken(channel.id, token.trim())
        }

        onOpenChange(false)
        onChannelUpdated()
        if (launchWelinkAuth && channel.channel_type === "welink") {
          onOpenWelinkAuth?.(channel.id)
        } else {
          toast.success(t("editSuccess"))
        }
      } catch (err: unknown) {
        const msg = toErrorMessage(err)
        setError(msg)
      } finally {
        setLoading(false)
      }
    },
    [
      name,
      token,
      chatId,
      channel,
      config,
      appId,
      baseUrl,
      welinkGatewayUrl,
      welinkAssistantName,
      welinkAk,
      welinkSk,
      topicMode,
      dailyReportEnabled,
      dailyReportTime,
      onOpenChange,
      onChannelUpdated,
      onOpenWelinkAuth,
      t,
    ]
  )

  const handleSubmit = useCallback(() => submitUpdate(false), [submitUpdate])

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{t("editChannel")}</DialogTitle>
        </DialogHeader>

        <div className="space-y-4">
          <div className="space-y-1.5">
            <label className="text-xs font-medium">{t("channelName")}</label>
            <Input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder={t("channelNamePlaceholder")}
            />
          </div>

          {channel.channel_type === "lark" && (
            <div className="space-y-1.5">
              <label className="text-xs font-medium">App ID</label>
              <Input
                value={appId}
                onChange={(e) => setAppId(e.target.value)}
                placeholder="cli_xxxxx"
              />
            </div>
          )}

          {channel.channel_type !== "weixin" &&
            channel.channel_type !== "welink" && (
              <div className="space-y-1.5">
                <label className="text-xs font-medium">
                  {channel.channel_type === "telegram"
                    ? "Bot Token"
                    : "App Secret"}
                </label>
                <Input
                  type="password"
                  value={token}
                  onChange={(e) => setToken(e.target.value)}
                  placeholder={
                    hasToken ? t("tokenPlaceholderKeep") : t("tokenRequired")
                  }
                />
              </div>
            )}

          {channel.channel_type !== "weixin" &&
            channel.channel_type !== "welink" && (
              <div className="space-y-1.5">
                <label className="text-xs font-medium">Chat ID</label>
                <Input
                  value={chatId}
                  onChange={(e) => setChatId(e.target.value)}
                  placeholder={
                    channel.channel_type === "telegram"
                      ? "-100123456789"
                      : "oc_xxxxx"
                  }
                />
              </div>
            )}

          {channel.channel_type === "welink" && (
            <>
              <div className="space-y-1.5">
                <label className="text-xs font-medium">
                  {t("welinkGatewayUrlLabel")}
                </label>
                <Input
                  value={welinkGatewayUrl}
                  onChange={(e) => setWelinkGatewayUrl(e.target.value)}
                  placeholder={t("welinkGatewayUrlPlaceholder")}
                />
                <p className="text-xs text-muted-foreground">
                  {t("welinkGatewayUrlHint")}
                </p>
              </div>

              <div className="space-y-1.5">
                <label className="text-xs font-medium">
                  {t("welinkAssistantNameLabel")}
                </label>
                <Input
                  value={welinkAssistantName}
                  onChange={(e) => setWelinkAssistantName(e.target.value)}
                  placeholder="codeagent"
                />
              </div>

              <div className="space-y-1.5">
                <label className="text-xs font-medium">
                  {t("welinkAkLabel")}
                </label>
                <Input
                  value={welinkAk}
                  onChange={(e) => setWelinkAk(e.target.value)}
                  placeholder={t("welinkAkPlaceholder")}
                />
              </div>

              <div className="space-y-1.5">
                <label className="text-xs font-medium">
                  {t("welinkSkLabel")}
                </label>
                <Input
                  type="password"
                  value={welinkSk}
                  onChange={(e) => setWelinkSk(e.target.value)}
                  placeholder={
                    hasToken
                      ? t("tokenPlaceholderKeep")
                      : t("welinkSkPlaceholder")
                  }
                />
              </div>
            </>
          )}

          {channel.channel_type === "telegram" && (
            <div className="rounded-md border border-border/70 p-3">
              <div className="flex items-center justify-between gap-3">
                <div>
                  <label htmlFor={topicModeId} className="text-xs font-medium">
                    {t("topicMode")}
                  </label>
                  <p className="mt-1 text-xs text-muted-foreground">
                    {t("topicModeHint")}
                  </p>
                </div>
                <Switch
                  id={topicModeId}
                  checked={topicMode}
                  onCheckedChange={setTopicMode}
                />
              </div>
            </div>
          )}

          {channel.channel_type === "weixin" && baseUrl && (
            <div className="space-y-1.5">
              <label className="text-xs font-medium">Base URL</label>
              <Input value={baseUrl} disabled />
            </div>
          )}

          <div className="flex items-center justify-between">
            <label className="text-xs font-medium">{t("dailyReport")}</label>
            <Switch
              checked={dailyReportEnabled}
              onCheckedChange={setDailyReportEnabled}
            />
          </div>

          {dailyReportEnabled && (
            <div className="space-y-1.5">
              <label className="text-xs font-medium">
                {t("dailyReportTime")}
              </label>
              <Input
                type="time"
                value={dailyReportTime}
                onChange={(e) => setDailyReportTime(e.target.value)}
              />
            </div>
          )}

          {error && (
            <div className="rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-400">
              {error}
            </div>
          )}
        </div>

        <DialogFooter>
          <Button
            variant="outline"
            onClick={() => onOpenChange(false)}
            disabled={loading}
          >
            {t("cancel")}
          </Button>
          {channel.channel_type === "welink" && onOpenWelinkAuth && (
            <Button
              variant="secondary"
              onClick={() => submitUpdate(true)}
              disabled={loading}
            >
              {loading && <Loader2 className="h-3.5 w-3.5 animate-spin mr-1" />}
              {t("welinkScanToGetCredentials")}
            </Button>
          )}
          <Button onClick={handleSubmit} disabled={loading}>
            {loading && <Loader2 className="h-3.5 w-3.5 animate-spin mr-1" />}
            {t("save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
