"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import {
  AlertCircle,
  Check,
  Copy,
  ExternalLink,
  Loader2,
  RefreshCw,
} from "lucide-react"
import { useTranslations } from "next-intl"

import { BrowserLink } from "@/components/ui/browser-link"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { welinkCheckRegisterStatus, welinkGetRegisterUrl } from "@/lib/api"
import { toErrorMessage } from "@/lib/app-error"

/** Fallback client-side auth expiry (5 minutes). */
const AUTH_EXPIRY_FALLBACK_MS = 5 * 60 * 1000
/** Polling interval for registration status (4s). */
const POLL_INTERVAL_MS = 4000
/** Show a warning banner after this many consecutive polling failures. */
const POLL_ERROR_WARN_THRESHOLD = 3

type AuthStatus = "loading" | "waiting" | "scanned" | "cancelled" | "expired"

interface WelinkAuthDialogProps {
  open: boolean
  channelId: number
  onOpenChange: (open: boolean) => void
  onAuthSuccess: (channelId: number) => void
}

function parseExpiryMs(expireTime: string): number {
  const numeric = Number(expireTime)
  if (Number.isFinite(numeric) && numeric > 0) {
    const ms = numeric < 1e12 ? numeric * 1000 : numeric
    const remaining = ms - Date.now()
    if (remaining > 5000 && remaining <= 30 * 60 * 1000) {
      return remaining
    }
  }
  const parsed = Date.parse(expireTime)
  if (!Number.isNaN(parsed)) {
    const remaining = parsed - Date.now()
    if (remaining > 5000 && remaining <= 30 * 60 * 1000) {
      return remaining
    }
  }
  return AUTH_EXPIRY_FALLBACK_MS
}

function WelinkAuthContent({
  channelId,
  onAuthSuccess,
  onClose,
}: {
  channelId: number
  onAuthSuccess: (channelId: number) => void
  onClose: () => void
}) {
  const t = useTranslations("ChatChannelSettings")
  const [pcUrl, setPcUrl] = useState<string | null>(null)
  const [session, setSession] = useState<{
    qrcode: string
    accessToken: string
    expiryMs: number
  } | null>(null)
  const [status, setStatus] = useState<AuthStatus>("loading")
  const [error, setError] = useState<string | null>(null)
  const [pollErrors, setPollErrors] = useState(0)
  const [copied, setCopied] = useState(false)

  const pollingRef = useRef<ReturnType<typeof setInterval> | null>(null)
  const expiryRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const copiedTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  const onAuthSuccessRef = useRef(onAuthSuccess)
  const onCloseRef = useRef(onClose)
  useEffect(() => {
    onAuthSuccessRef.current = onAuthSuccess
    onCloseRef.current = onClose
  })

  const stopPolling = useCallback(() => {
    if (pollingRef.current) {
      clearInterval(pollingRef.current)
      pollingRef.current = null
    }
    if (expiryRef.current) {
      clearTimeout(expiryRef.current)
      expiryRef.current = null
    }
  }, [])

  const fetchRegisterUrl = useCallback(async () => {
    setStatus("loading")
    setError(null)
    setPcUrl(null)
    setSession(null)
    setPollErrors(0)
    setCopied(false)
    stopPolling()

    try {
      const result = await welinkGetRegisterUrl({ channelId })
      setPcUrl(result.pcUrl)
      setSession({
        qrcode: result.qrcode,
        accessToken: result.accessToken,
        expiryMs: parseExpiryMs(result.expireTime),
      })
      setStatus("waiting")
    } catch (err) {
      setError(toErrorMessage(err))
      setStatus("expired")
    }
  }, [channelId, stopPolling])

  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- initial data fetch on mount
    fetchRegisterUrl()
    return () => {
      stopPolling()
      if (copiedTimerRef.current) {
        clearTimeout(copiedTimerRef.current)
        copiedTimerRef.current = null
      }
    }
  }, [fetchRegisterUrl, stopPolling])

  useEffect(() => {
    if (!session) return

    expiryRef.current = setTimeout(() => {
      stopPolling()
      setStatus("expired")
    }, session.expiryMs)

    pollingRef.current = setInterval(async () => {
      try {
        const result = await welinkCheckRegisterStatus(
          channelId,
          session.qrcode,
          session.accessToken
        )
        setPollErrors(0)
        if (result.status === "confirmed") {
          stopPolling()
          onAuthSuccessRef.current(channelId)
          onCloseRef.current()
        } else if (result.status === "scanned") {
          setStatus("scanned")
        } else if (result.status === "cancelled") {
          stopPolling()
          setStatus("cancelled")
        } else if (result.status === "expired") {
          stopPolling()
          setStatus("expired")
        }
      } catch {
        setPollErrors((n) => n + 1)
      }
    }, POLL_INTERVAL_MS)

    return () => stopPolling()
  }, [session, channelId, stopPolling])

  const handleCopy = useCallback(async () => {
    if (!pcUrl) return
    try {
      await navigator.clipboard.writeText(pcUrl)
      setCopied(true)
      if (copiedTimerRef.current) clearTimeout(copiedTimerRef.current)
      copiedTimerRef.current = setTimeout(() => setCopied(false), 2000)
    } catch {
      // Clipboard API may be blocked in insecure contexts
    }
  }, [pcUrl])

  const isActive = status === "waiting" || status === "scanned"

  return (
    <div className="flex flex-col items-center gap-4 py-4">
      <p className="text-sm text-muted-foreground text-center">
        {t("welinkAuthDescription")}
      </p>

      {status === "loading" && (
        <div className="flex h-32 w-full items-center justify-center">
          <Loader2 className="h-8 w-8 animate-spin text-muted-foreground" />
        </div>
      )}

      {isActive && pcUrl && (
        <div className="w-full space-y-3">
          <div className="rounded-md border bg-muted/40 p-3 text-xs break-all font-mono">
            {pcUrl}
          </div>
          <div className="flex items-center justify-center gap-2">
            <Button asChild size="sm">
              <BrowserLink href={pcUrl}>
                <ExternalLink className="h-3.5 w-3.5 mr-1.5" />
                {t("welinkOpenAuthLink")}
              </BrowserLink>
            </Button>
            <Button variant="outline" size="sm" onClick={handleCopy}>
              {copied ? (
                <>
                  <Check className="h-3.5 w-3.5 mr-1.5" />
                  {t("welinkPcUrlCopied")}
                </>
              ) : (
                <>
                  <Copy className="h-3.5 w-3.5 mr-1.5" />
                  {t("welinkPcUrlCopy")}
                </>
              )}
            </Button>
          </div>
          <p className="flex items-center justify-center gap-2 text-xs text-muted-foreground">
            <Loader2 className="h-3 w-3 animate-spin" />
            {status === "scanned" ? t("welinkScanned") : t("welinkWaitingAuth")}
          </p>
          {pollErrors >= POLL_ERROR_WARN_THRESHOLD && (
            <div className="flex items-center justify-center gap-1.5 rounded-md border border-yellow-500/30 bg-yellow-500/5 px-3 py-1.5 text-xs text-yellow-500">
              <AlertCircle className="h-3.5 w-3.5 shrink-0" />
              {t("welinkPollError")}
            </div>
          )}
        </div>
      )}

      {(status === "expired" || status === "cancelled") && (
        <>
          <div className="flex h-24 w-full items-center justify-center rounded-md bg-muted px-4">
            <p className="text-sm text-muted-foreground text-center">
              {status === "cancelled"
                ? t("welinkAuthCancelled")
                : t("welinkAuthExpired")}
            </p>
          </div>
          <Button variant="outline" size="sm" onClick={fetchRegisterUrl}>
            <RefreshCw className="h-3.5 w-3.5 mr-1" />
            {t("welinkRefreshAuth")}
          </Button>
        </>
      )}

      {error && (
        <div className="w-full rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-400">
          {error}
        </div>
      )}
    </div>
  )
}

export function WelinkAuthDialog({
  open,
  channelId,
  onOpenChange,
  onAuthSuccess,
}: WelinkAuthDialogProps) {
  const t = useTranslations("ChatChannelSettings")
  const handleClose = useCallback(() => onOpenChange(false), [onOpenChange])

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{t("welinkAuthTitle")}</DialogTitle>
        </DialogHeader>
        {open && (
          <WelinkAuthContent
            channelId={channelId}
            onAuthSuccess={onAuthSuccess}
            onClose={handleClose}
          />
        )}
      </DialogContent>
    </Dialog>
  )
}
