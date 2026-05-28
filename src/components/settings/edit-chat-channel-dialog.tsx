"use client"

import { useCallback, useEffect, useState } from "react"
import { Loader2 } from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Textarea } from "@/components/ui/textarea"
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
}

export function EditChatChannelDialog({
  open,
  channel,
  onOpenChange,
  onChannelUpdated,
}: EditChatChannelDialogProps) {
  const t = useTranslations("ChatChannelSettings")
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const config = JSON.parse(channel.config_json || "{}")
  const [name, setName] = useState(channel.name)
  const [token, setToken] = useState("")
  const [chatId, setChatId] = useState(config.chat_id ?? "")
  const [appId, setAppId] = useState(config.app_id ?? "")
  const [baseUrl] = useState(config.base_url ?? "")
  const [welinkGroupId, setWelinkGroupId] = useState(config.group_id ?? "")
  const [welinkSendHttpUrl, setWelinkSendHttpUrl] = useState(
    config.send_http_url ?? ""
  )
  const [welinkCliPath, setWelinkCliPath] = useState(
    config.welink_cli_path ?? "welink-cli"
  )
  const [includeSender, setIncludeSender] = useState(
    arrayToLines(config.include_sender)
  )
  const [excludeSender, setExcludeSender] = useState(
    arrayToLines(config.exclude_sender)
  )
  const [dailyReportEnabled, setDailyReportEnabled] = useState(
    channel.daily_report_enabled
  )
  const [dailyReportTime, setDailyReportTime] = useState(
    channel.daily_report_time || "18:00"
  )
  const [hasToken, setHasToken] = useState(false)

  useEffect(() => {
    if (open) {
      getChatChannelHasToken(channel.id)
        .then(setHasToken)
        .catch(() => {})
    }
  }, [open, channel.id])

  const handleSubmit = useCallback(async () => {
    if (!name.trim()) {
      setError(t("nameRequired"))
      return
    }
    if (
      (channel.channel_type === "telegram" ||
        channel.channel_type === "lark") &&
      !chatId.trim()
    ) {
      setError(t("chatIdRequired"))
      return
    }
    if (channel.channel_type === "welink") {
      if (!welinkGroupId.trim()) {
        setError(t("welinkGroupIdRequired"))
        return
      }
      if (!welinkSendHttpUrl.trim()) {
        setError(t("welinkSendHttpUrlRequired"))
        return
      }
      if (!welinkCliPath.trim()) {
        setError(t("welinkCliPathRequired"))
        return
      }
    }

    setLoading(true)
    setError(null)
    try {
      const configJson =
        channel.channel_type === "weixin"
          ? JSON.stringify({ base_url: baseUrl })
          : channel.channel_type === "welink"
            ? JSON.stringify({
                group_id: welinkGroupId.trim(),
                send_http_url: welinkSendHttpUrl.trim(),
                welink_cli_path: welinkCliPath.trim(),
                include_sender: linesToArray(includeSender),
                exclude_sender: linesToArray(excludeSender),
              })
            : channel.channel_type === "lark"
              ? JSON.stringify({ app_id: appId, chat_id: chatId })
              : JSON.stringify({ chat_id: chatId })

      await updateChatChannel({
        id: channel.id,
        name: name.trim(),
        configJson,
        dailyReportEnabled,
        dailyReportTime: dailyReportEnabled ? dailyReportTime : null,
      })

      if (token.trim()) {
        await saveChatChannelToken(channel.id, token.trim())
      }

      onOpenChange(false)
      onChannelUpdated()
      toast.success(t("editSuccess"))
    } catch (err: unknown) {
      const msg = toErrorMessage(err)
      setError(msg)
    } finally {
      setLoading(false)
    }
  }, [
    name,
    token,
    chatId,
    channel,
    appId,
    baseUrl,
    welinkGroupId,
    welinkSendHttpUrl,
    welinkCliPath,
    includeSender,
    excludeSender,
    dailyReportEnabled,
    dailyReportTime,
    onOpenChange,
    onChannelUpdated,
    t,
  ])

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

          {channel.channel_type !== "weixin" && (
            <div className="space-y-1.5">
              <label className="text-xs font-medium">
                {channel.channel_type === "telegram"
                  ? "Bot Token"
                  : channel.channel_type === "welink"
                    ? t("welinkToken")
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

          {(channel.channel_type === "telegram" ||
            channel.channel_type === "lark") && (
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
                  {t("welinkGroupId")}
                </label>
                <Input
                  value={welinkGroupId}
                  onChange={(e) => setWelinkGroupId(e.target.value)}
                  placeholder="957084088626496500"
                />
              </div>

              <div className="space-y-1.5">
                <label className="text-xs font-medium">
                  {t("welinkSendHttpUrl")}
                </label>
                <Input
                  value={welinkSendHttpUrl}
                  onChange={(e) => setWelinkSendHttpUrl(e.target.value)}
                  placeholder="http://xiaoluban.rnd.example.com:80/"
                />
              </div>

              <div className="space-y-1.5">
                <label className="text-xs font-medium">
                  {t("welinkCliPath")}
                </label>
                <Input
                  value={welinkCliPath}
                  onChange={(e) => setWelinkCliPath(e.target.value)}
                  placeholder="welink-cli"
                />
              </div>

              <div className="space-y-1.5">
                <label className="text-xs font-medium">
                  {t("includeSender")}
                </label>
                <Textarea
                  value={includeSender}
                  onChange={(e) => setIncludeSender(e.target.value)}
                  placeholder={t("senderListPlaceholder")}
                  className="min-h-20"
                />
              </div>

              <div className="space-y-1.5">
                <label className="text-xs font-medium">
                  {t("excludeSender")}
                </label>
                <Textarea
                  value={excludeSender}
                  onChange={(e) => setExcludeSender(e.target.value)}
                  placeholder={t("senderListPlaceholder")}
                  className="min-h-20"
                />
              </div>
            </>
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
          <Button onClick={handleSubmit} disabled={loading}>
            {loading && <Loader2 className="h-3.5 w-3.5 animate-spin mr-1" />}
            {t("save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function linesToArray(value: string): string[] {
  return value
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
}

function arrayToLines(value: unknown): string {
  return Array.isArray(value)
    ? value
        .filter((item): item is string => typeof item === "string")
        .join("\n")
    : ""
}
