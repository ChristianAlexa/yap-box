import { useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

type Phase = 'idle' | 'confirming' | 'downloading' | 'error' | 'complete'

interface ProgressPayload {
  file: 'onnx' | 'voices'
  downloaded: number
  total: number
  overall_pct: number
}

interface ErrorPayload {
  file: string
  message: string
  cancelled: boolean
}

interface Props {
  onComplete: () => void
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / (1024 * 1024)).toFixed(1)} MB`
}

export function ModelDownload({ onComplete }: Props) {
  const [phase, setPhase] = useState<Phase>('idle')
  const [progress, setProgress] = useState<ProgressPayload | null>(null)
  const [errorMsg, setErrorMsg] = useState<string | null>(null)
  const unlistenersRef = useRef<UnlistenFn[]>([])

  useEffect(() => {
    const setup = async () => {
      const u1 = await listen<ProgressPayload>('model-download-progress', (e) => {
        setProgress(e.payload)
      })
      const u2 = await listen<unknown>('model-download-complete', () => {
        setPhase('complete')
        onComplete()
      })
      const u3 = await listen<ErrorPayload>('model-download-error', (e) => {
        if (e.payload.cancelled) {
          setPhase('idle')
          setProgress(null)
          setErrorMsg(null)
        } else {
          setPhase('error')
          setErrorMsg(`${e.payload.file}: ${e.payload.message}`)
        }
      })
      unlistenersRef.current = [u1, u2, u3]
    }
    setup()
    return () => {
      unlistenersRef.current.forEach((fn) => fn())
      unlistenersRef.current = []
    }
  }, [onComplete])

  const startDownload = async () => {
    setErrorMsg(null)
    setProgress(null)
    setPhase('downloading')
    try {
      await invoke('download_model')
    } catch (err) {
      setPhase('error')
      setErrorMsg(`${err}`)
    }
  }

  const cancelDownload = async () => {
    try {
      await invoke('cancel_download')
    } catch {
      // best-effort
    }
  }

  if (phase === 'confirming') {
    return (
      <div className="model-gate">
        <div className="model-dialog">
          <h2 className="model-dialog__title">Download Kokoro model?</h2>
          <p className="model-dialog__body">
            About <strong>354 MB</strong> will be downloaded from GitHub to{' '}
            <code>~/Library/Application Support/com.yapbox.app/models/</code>. This may take several
            minutes depending on your connection.
          </p>
          <div className="model-dialog__actions">
            <button type="button" className="button--secondary" onClick={() => setPhase('idle')}>
              Cancel
            </button>
            <button type="button" onClick={startDownload}>
              Download
            </button>
          </div>
        </div>
      </div>
    )
  }

  if (phase === 'downloading') {
    const pct = progress?.overall_pct ?? 0
    const fileLabel = progress?.file === 'voices' ? 'voices-v1.0.bin' : 'kokoro-v1.0.onnx'
    return (
      <div className="model-gate">
        <div className="model-card">
          <h2 className="model-card__title">Downloading Kokoro model</h2>
          <div className="progress">
            <div className="progress__bar" style={{ width: `${pct}%` }} />
          </div>
          <p className="model-card__meta">
            {fileLabel} •{' '}
            {progress
              ? `${formatBytes(progress.downloaded)} / ${formatBytes(progress.total)}`
              : 'Starting...'}
          </p>
          <button type="button" className="button--secondary" onClick={cancelDownload}>
            Cancel
          </button>
        </div>
      </div>
    )
  }

  if (phase === 'error') {
    return (
      <div className="model-gate">
        <div className="model-card">
          <h2 className="model-card__title">Download failed</h2>
          <p className="error">{errorMsg}</p>
          <button type="button" onClick={() => setPhase('confirming')}>
            Retry
          </button>
        </div>
      </div>
    )
  }

  if (phase === 'complete') {
    return (
      <div className="model-gate">
        <div className="model-card">
          <h2 className="model-card__title">Model ready</h2>
          <p className="model-card__meta">Starting Kokoros...</p>
        </div>
      </div>
    )
  }

  return (
    <div className="model-gate">
      <div className="model-card">
        <h2 className="model-card__title">Kokoro model not installed</h2>
        <p className="model-card__body">
          yap-box needs the Kokoro-82M TTS model (~354 MB, Apache 2.0) to generate speech. The model
          will be stored at <code>~/Library/Application Support/com.yapbox.app/models/</code>.
        </p>
        <button type="button" onClick={() => setPhase('confirming')}>
          Download model
        </button>
      </div>
    </div>
  )
}
