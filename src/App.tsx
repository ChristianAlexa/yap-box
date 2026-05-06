import { useState, useEffect, useCallback } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { ModelDownload } from './components/ModelDownload'
import { Settings } from './components/Settings'
import { useSpinner } from './hooks/useSpinner'

interface SpeakResult {
  duration_ms: number
  char_count: number
  stopped: boolean
}

interface ModelStatus {
  present: boolean
  onnx_bytes: number | null
  voices_bytes: number | null
}

type Gate =
  | { kind: 'loading' }
  | { kind: 'need-model' }
  | { kind: 'starting' }
  | { kind: 'ready' }
  | { kind: 'engine-error'; message: string }

function App() {
  const [gate, setGate] = useState<Gate>({ kind: 'loading' })
  const [text, setText] = useState('')
  const [speaking, setSpeaking] = useState(false)
  const [paused, setPaused] = useState(false)
  const [previewing, setPreviewing] = useState(false)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [voices, setVoices] = useState<string[]>([])
  const [selectedVoice, setSelectedVoice] = useState<string>(
    () => localStorage.getItem('yap-box-voice') ?? 'af_heart',
  )
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null)
  const spinnerFrame = useSpinner(speaking)

  useEffect(() => {
    const unlisten = listen<{ done: number; total: number }>('speak-progress', (event) => {
      setProgress(event.payload)
    })
    return () => {
      unlisten.then((fn) => fn())
    }
  }, [])

  useEffect(() => {
    let cancelled = false
    ;(async () => {
      try {
        const status = await invoke<ModelStatus>('model_status')
        if (cancelled) return
        if (!status.present) {
          setGate({ kind: 'need-model' })
        } else {
          setGate({ kind: 'starting' })
          await invoke('start_kokoros')
          if (!cancelled) setGate({ kind: 'ready' })
        }
      } catch (err) {
        if (!cancelled) setGate({ kind: 'engine-error', message: `${err}` })
      }
    })()
    return () => {
      cancelled = true
    }
  }, [])

  useEffect(() => {
    if (gate.kind !== 'ready') return
    let cancelled = false
    ;(async () => {
      try {
        const list = await invoke<string[]>('list_voices')
        if (!cancelled) setVoices(list)
      } catch {
        // keep select empty-ish
      }
    })()
    return () => {
      cancelled = true
    }
  }, [gate.kind])

  useEffect(() => {
    localStorage.setItem('yap-box-voice', selectedVoice)
  }, [selectedVoice])

  useEffect(() => {
    const unlisten = listen<{ paths: string[] }>('tauri://drag-drop', async (event) => {
      const path = event.payload.paths[0]
      if (!path) return
      try {
        const contents: string = await invoke('read_file', { path })
        setText(contents)
        setError(null)
      } catch (err) {
        setError(`Failed to read file: ${err}`)
      }
    })
    return () => {
      unlisten.then((fn) => fn())
    }
  }, [])

  useEffect(() => {
    const unlisten = listen<number | null>('kokoros-terminated', (event) => {
      setGate({
        kind: 'engine-error',
        message: `Kokoros exited unexpectedly (code ${event.payload ?? 'unknown'}).`,
      })
    })
    return () => {
      unlisten.then((fn) => fn())
    }
  }, [])

  async function handleYap() {
    if (!text.trim() || speaking) return
    setSpeaking(true)
    setError(null)
    setProgress(null)
    try {
      const result = await invoke<SpeakResult>('speak', { text, voice: selectedVoice })
      if (!result.stopped) {
        console.log(`Spoke ${result.char_count} chars in ${result.duration_ms}ms`)
      }
    } catch (err) {
      setError(`${err}`)
    } finally {
      setSpeaking(false)
      setPaused(false)
      setProgress(null)
    }
  }

  async function handleStop() {
    try {
      await invoke('stop')
    } catch {
      // best-effort
    }
  }

  async function handlePreview() {
    if (previewing || speaking) return
    setPreviewing(true)
    setError(null)
    try {
      await invoke('preview_voice', { voice: selectedVoice })
    } catch (err) {
      setError(`${err}`)
    } finally {
      setPreviewing(false)
    }
  }

  async function handlePauseToggle() {
    try {
      if (paused) {
        await invoke('resume')
        setPaused(false)
      } else {
        await invoke('pause')
        setPaused(true)
      }
    } catch {
      // best-effort
    }
  }

  const handleModelReady = useCallback(async () => {
    setGate({ kind: 'starting' })
    try {
      await invoke('start_kokoros')
      setGate({ kind: 'ready' })
    } catch (err) {
      setGate({ kind: 'engine-error', message: `${err}` })
    }
  }, [])

  if (gate.kind === 'loading') {
    return (
      <div className="container">
        <div className="model-gate">
          <p className="model-card__meta">Loading...</p>
        </div>
      </div>
    )
  }

  if (gate.kind === 'need-model') {
    return (
      <div className="container">
        <ModelDownload onComplete={handleModelReady} />
      </div>
    )
  }

  if (gate.kind === 'starting') {
    return (
      <div className="container">
        <div className="model-gate">
          <div className="model-card">
            <h2 className="model-card__title">Starting Kokoros...</h2>
            <p className="model-card__meta">First launch may take a few seconds.</p>
          </div>
        </div>
      </div>
    )
  }

  if (gate.kind === 'engine-error') {
    return (
      <div className="container">
        <div className="model-gate">
          <div className="model-card">
            <h2 className="model-card__title">Kokoros failed to start</h2>
            <p className="error">{gate.message}</p>
            <button
              onClick={async () => {
                setGate({ kind: 'starting' })
                try {
                  await invoke('start_kokoros')
                  setGate({ kind: 'ready' })
                } catch (err) {
                  setGate({ kind: 'engine-error', message: `${err}` })
                }
              }}
            >
              Retry
            </button>
          </div>
        </div>
      </div>
    )
  }

  if (settingsOpen) {
    return (
      <div className="container">
        <Settings
          voices={voices}
          selectedVoice={selectedVoice}
          setSelectedVoice={setSelectedVoice}
          speaking={speaking}
          previewing={previewing}
          onPreview={handlePreview}
          engineConnected={gate.kind === 'ready'}
          onClose={() => setSettingsOpen(false)}
        />
      </div>
    )
  }

  return (
    <div className="container">
      <header className="header">
        <h1>yap-box</h1>
        <div className="header__right">
          <button
            className="icon-button"
            onClick={() => setSettingsOpen(true)}
            aria-label="Settings"
            title="Settings"
          >
            ⚙
          </button>
        </div>
      </header>
      <textarea
        value={text}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && e.metaKey) {
            e.preventDefault()
            handleYap()
          }
        }}
        placeholder="Paste text here or drag a file onto this window..."
        rows={12}
      />
      {speaking && progress && progress.total > 0 && (
        <div className="yap-status" aria-live="polite">
          <div className="yap-status__row">
            <span className="yap-status__spinner" aria-hidden="true">
              {spinnerFrame}
            </span>
            <span className="yap-status__label">
              Yapping… {Math.min(progress.total, Math.floor(progress.done) + 1)} of {progress.total}
            </span>
          </div>
          <div className="progress">
            <div
              className="progress__bar"
              style={{ width: `${(progress.done / progress.total) * 100}%` }}
            />
          </div>
        </div>
      )}
      {speaking ? (
        <div className="button-row">
          <button onClick={handlePauseToggle}>
            {paused ? 'Resume' : 'Pause'}
          </button>
          <button onClick={handleStop} className="button--stop">
            Stop
          </button>
        </div>
      ) : (
        <button onClick={handleYap} disabled={!text.trim()}>
          Yap
        </button>
      )}
      {error && <p className="error">{error}</p>}
    </div>
  )
}

export default App
