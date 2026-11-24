"use client"

import { useCallback, useId, useState } from "react"
import { Loader2 } from "lucide-react"
import { useTranslations } from "next-intl"

import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
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
  onOpenWelinkAuth?: (channelId: number) => void
}

export function AddChatChannelDialog({
  open,
  onOpenChange,
  onChannelAdded,
  onOpenWelinkAuth,
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
  const [welinkGatewayUrl, setWelinkGatewayUrl] = useState("")
  const [welinkAssistantName, setWelinkAssistantName] = useState("codeagent")
  const [welinkAk, setWelinkAk] = useState("")
  const [welinkSk, setWelinkSk] = useState("")
  const [topicMode, setTopicMode] = useState(false)
  const [dailyReportEnabled, setDailyReportEnabled] = useState(false)
  const [dailyReportTime, setDailyReportTime] = useState("18:00")
  const topicModeId = useId()

  const resetForm = useCallback(() => {
    setName("")
    setChannelType("telegram")
    setToken("")
    setChatId("")
    setAppId("")
    setBaseUrl("https://ilinkai.weixin.qq.com")
    setWelinkGatewayUrl("")
    setWelinkAssistantName("codeagent")
    setWelinkAk("")
    setWelinkSk("")
    setTopicMode(false)
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

  const submitChannel = useCallback(
    async (launchWelinkAuth: boolean) => {
      if (!name.trim()) {
        setError(t("nameRequired"))
        return
      }
      if (channelType === "welink") {
        if (!welinkGatewayUrl.trim()) {
          setError(t("welinkGatewayUrlRequired"))
          return
        }
      } else if (channelType !== "weixin") {
        if (!token.trim()) {
          setError(t("tokenRequired"))
          return
        }
        if (!chatId.trim()) {
          setError(t("chatIdRequired"))
          return
        }
      }

      setLoading(true)
      setError(null)
      try {
        const configJson =
          channelType === "weixin"
            ? JSON.stringify({ base_url: baseUrl })
            : channelType === "lark"
              ? JSON.stringify({ app_id: appId, chat_id: chatId })
              : channelType === "welink"
                ? JSON.stringify({
                    assistant_name: welinkAssistantName.trim() || "codeagent",
                    gateway_url: welinkGatewayUrl.trim(),
                    ak: welinkAk.trim(),
                  })
                : JSON.stringify({ chat_id: chatId, topic_mode: topicMode })

        const channel = await createChatChannel({
          name: name.trim(),
          channelType,
          configJson,
          enabled: true,
          dailyReportEnabled,
          dailyReportTime: dailyReportEnabled ? dailyReportTime : null,
        })

        if (channelType === "welink") {
          if (welinkSk.trim()) {
            await saveChatChannelToken(channel.id, welinkSk.trim())
          }
        } else if (channelType !== "weixin" && token.trim()) {
          await saveChatChannelToken(channel.id, token.trim())
        }

        handleOpenChange(false)
        onChannelAdded()
        if (launchWelinkAuth && channelType === "welink") {
          onOpenWelinkAuth?.(channel.id)
        }
      } catch (err) {
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
      channelType,
      appId,
      baseUrl,
      welinkGatewayUrl,
      welinkAssistantName,
      welinkAk,
      welinkSk,
      topicMode,
      dailyReportEnabled,
      dailyReportTime,
      handleOpenChange,
      onChannelAdded,
      onOpenWelinkAuth,
      t,
    ]
  )

  const handleSubmit = useCallback(() => submitChannel(false), [submitChannel])

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
                <SelectItem value="weixin">{t("weixin")}</SelectItem>
                <SelectItem value="welink">{t("welink")}</SelectItem>
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

          {channelType !== "weixin" && channelType !== "welink" && (
            <div className="space-y-1.5">
              <label className="text-xs font-medium">
                {channelType === "telegram" ? "Bot Token" : "App Secret"}
              </label>
              <Input
                type="password"
                value={token}
                onChange={(e) => setToken(e.target.value)}
                placeholder={
                  channelType === "telegram" ? "123456:ABC-DEF..." : "xxxxx"
                }
              />
            </div>
          )}

          {channelType !== "weixin" && channelType !== "welink" && (
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
                  placeholder={t("welinkSkPlaceholder")}
                />
                <p className="text-xs text-muted-foreground">
                  {t("welinkCredentialsOptionalHint")}
                </p>
              </div>
            </>
          )}

          {channelType === "telegram" && (
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
          {channelType === "welink" &&
            onOpenWelinkAuth &&
            !welinkAk.trim() &&
            !welinkSk.trim() && (
              <Button
                variant="secondary"
                onClick={() => submitChannel(true)}
                disabled={loading}
              >
                {loading && (
                  <Loader2 className="h-3.5 w-3.5 animate-spin mr-1" />
                )}
                {t("welinkScanToGetCredentials")}
              </Button>
            )}
          <Button onClick={handleSubmit} disabled={loading}>
            {loading && <Loader2 className="h-3.5 w-3.5 animate-spin mr-1" />}
            {t("create")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
