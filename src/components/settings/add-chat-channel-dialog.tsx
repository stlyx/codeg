"use client"

import { useCallback, useState } from "react"
import { Loader2 } from "lucide-react"
import { useTranslations } from "next-intl"

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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Switch } from "@/components/ui/switch"
import { createChatChannel, saveChatChannelToken } from "@/lib/api"
import type { ChannelType } from "@/lib/types"
import { toErrorMessage } from "@/lib/app-error"

interface AddChatChannelDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  onChannelAdded: () => void
}

export function AddChatChannelDialog({
  open,
  onOpenChange,
  onChannelAdded,
}: AddChatChannelDialogProps) {
  const t = useTranslations("ChatChannelSettings")
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const [name, setName] = useState("")
  const [channelType, setChannelType] = useState<ChannelType>("telegram")
  const [token, setToken] = useState("")
  const [chatId, setChatId] = useState("")
  const [appId, setAppId] = useState("")
  const [baseUrl, setBaseUrl] = useState("https://ilinkai.weixin.qq.com")
  const [welinkGroupId, setWelinkGroupId] = useState("")
  const [welinkSendHttpUrl, setWelinkSendHttpUrl] = useState("")
  const [welinkCliPath, setWelinkCliPath] = useState("welink-cli")
  const [includeSender, setIncludeSender] = useState("")
  const [excludeSender, setExcludeSender] = useState("")
  const [dailyReportEnabled, setDailyReportEnabled] = useState(false)
  const [dailyReportTime, setDailyReportTime] = useState("18:00")

  const resetForm = useCallback(() => {
    setName("")
    setChannelType("telegram")
    setToken("")
    setChatId("")
    setAppId("")
    setBaseUrl("https://ilinkai.weixin.qq.com")
    setWelinkGroupId("")
    setWelinkSendHttpUrl("")
    setWelinkCliPath("welink-cli")
    setIncludeSender("")
    setExcludeSender("")
    setDailyReportEnabled(false)
    setDailyReportTime("18:00")
    setError(null)
  }, [])

  const handleOpenChange = useCallback(
    (nextOpen: boolean) => {
      if (!nextOpen) resetForm()
      onOpenChange(nextOpen)
    },
    [onOpenChange, resetForm]
  )

  const handleSubmit = useCallback(async () => {
    if (!name.trim()) {
      setError(t("nameRequired"))
      return
    }
    if (channelType !== "weixin" && !token.trim()) {
      setError(t("tokenRequired"))
      return
    }
    if (
      (channelType === "telegram" || channelType === "lark") &&
      !chatId.trim()
    ) {
      setError(t("chatIdRequired"))
      return
    }
    if (channelType === "welink") {
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
        channelType === "weixin"
          ? JSON.stringify({ base_url: baseUrl })
          : channelType === "welink"
            ? JSON.stringify({
                group_id: welinkGroupId.trim(),
                send_http_url: welinkSendHttpUrl.trim(),
                welink_cli_path: welinkCliPath.trim(),
                include_sender: linesToArray(includeSender),
                exclude_sender: linesToArray(excludeSender),
              })
            : channelType === "lark"
              ? JSON.stringify({ app_id: appId, chat_id: chatId })
              : JSON.stringify({ chat_id: chatId })

      const channel = await createChatChannel({
        name: name.trim(),
        channelType,
        configJson,
        enabled: true,
        dailyReportEnabled,
        dailyReportTime: dailyReportEnabled ? dailyReportTime : null,
      })

      if (channelType !== "weixin" && token.trim()) {
        await saveChatChannelToken(channel.id, token.trim())
      }

      handleOpenChange(false)
      onChannelAdded()
    } catch (err) {
      const msg = toErrorMessage(err)
      setError(msg)
    } finally {
      setLoading(false)
    }
  }, [
    name,
    token,
    chatId,
    channelType,
    appId,
    baseUrl,
    welinkGroupId,
    welinkSendHttpUrl,
    welinkCliPath,
    includeSender,
    excludeSender,
    dailyReportEnabled,
    dailyReportTime,
    handleOpenChange,
    onChannelAdded,
    t,
  ])

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{t("addChannel")}</DialogTitle>
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

          <div className="space-y-1.5">
            <label className="text-xs font-medium">{t("channelType")}</label>
            <Select
              value={channelType}
              onValueChange={(v) => setChannelType(v as ChannelType)}
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="telegram">Telegram</SelectItem>
                <SelectItem value="lark">{t("lark")}</SelectItem>
                <SelectItem value="welink">{t("welink")}</SelectItem>
                <SelectItem value="weixin">{t("weixin")}</SelectItem>
              </SelectContent>
            </Select>
          </div>

          {channelType === "lark" && (
            <div className="space-y-1.5">
              <label className="text-xs font-medium">App ID</label>
              <Input
                value={appId}
                onChange={(e) => setAppId(e.target.value)}
                placeholder="cli_xxxxx"
              />
            </div>
          )}

          {channelType !== "weixin" && (
            <div className="space-y-1.5">
              <label className="text-xs font-medium">
                {channelType === "telegram"
                  ? "Bot Token"
                  : channelType === "welink"
                    ? t("welinkToken")
                    : "App Secret"}
              </label>
              <Input
                type="password"
                value={token}
                onChange={(e) => setToken(e.target.value)}
                placeholder={
                  channelType === "telegram"
                    ? "123456:ABC-DEF..."
                    : channelType === "welink"
                      ? "token"
                      : "xxxxx"
                }
              />
            </div>
          )}

          {(channelType === "telegram" || channelType === "lark") && (
            <div className="space-y-1.5">
              <label className="text-xs font-medium">Chat ID</label>
              <Input
                value={chatId}
                onChange={(e) => setChatId(e.target.value)}
                placeholder={
                  channelType === "telegram" ? "-100123456789" : "oc_xxxxx"
                }
              />
            </div>
          )}

          {channelType === "welink" && (
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

          {channelType === "weixin" && (
            <p className="text-xs text-muted-foreground">
              {t("weixinScanDescription")}
            </p>
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
            onClick={() => handleOpenChange(false)}
            disabled={loading}
          >
            {t("cancel")}
          </Button>
          <Button onClick={handleSubmit} disabled={loading}>
            {loading && <Loader2 className="h-3.5 w-3.5 animate-spin mr-1" />}
            {t("create")}
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
