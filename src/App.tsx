import { useState, useEffect } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

interface SpeakResult {
  duration_ms: number
  char_count: number
}

function App() {
  const [text, setText] = useState('')
  const [speaking, setSpeaking] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [kokorosUp, setKokorosUp] = useState<boolean | null>(null)
  const [voices, setVoices] = useState<string[]>([])
  const [selectedVoice, setSelectedVoice] = useState<string>(
    () => localStorage.getItem('yap-box-voice') ?? 'af_heart',
  )

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
    return () => { unlisten.then(fn => fn()) }
  }, [])

  useEffect(() => {
    let cancelled = false
    ;(async () => {
      try {
        const list = await invoke<string[]>('list_voices')
        if (!cancelled) setVoices(list)
      } catch {
        // keep select empty-ish; user can still press Yap with the default voice
      }
    })()
    return () => { cancelled = true }
  }, [])

  useEffect(() => {
    localStorage.setItem('yap-box-voice', selectedVoice)
  }, [selectedVoice])

  // One check on mount to seed the indicator.
  useEffect(() => {
    let cancelled = false
    ;(async () => {
      const up = await invoke<boolean>('kokoros_reachable')
      if (!cancelled) setKokorosUp(up)
    })()
    return () => { cancelled = true }
  }, [])

  // Only poll while Kokoros is known-down, so the help banner disappears
  // promptly after the user starts `koko openai`. No polling once it's up —
  // speak() calls update status as a side effect.
  useEffect(() => {
    if (kokorosUp !== false) return
    let cancelled = false
    const id = setInterval(async () => {
      const up = await invoke<boolean>('kokoros_reachable')
      if (!cancelled) setKokorosUp(up)
    }, 3000)
    return () => { cancelled = true; clearInterval(id) }
  }, [kokorosUp])

  async function handleYap() {
    if (!text.trim() || speaking) return
    setSpeaking(true)
    setError(null)
    try {
      const result = await invoke<SpeakResult>('speak', { text, voice: selectedVoice })
      console.log(`Spoke ${result.char_count} chars in ${result.duration_ms}ms`)
      setKokorosUp(true)
    } catch (err) {
      const msg = `${err}`
      // User-initiated stop: afplay killed via SIGTERM reports exit code
      // Some(15) or signal-based termination. Swallow silently.
      const looksStopped = msg.includes('Some(15)') || msg.includes('-15') || msg.includes('None')
      if (!looksStopped) setError(msg)
      if (msg.toLowerCase().includes('unreachable')) setKokorosUp(false)
    } finally {
      setSpeaking(false)
    }
  }

  async function handleStop() {
    try {
      await invoke('stop')
    } catch {
      // best-effort: nothing to surface
    }
  }

  const statusLabel =
    kokorosUp === null ? 'Checking Kokoros...' : kokorosUp ? 'Kokoros connected' : 'Kokoros not running'
  const statusClass =
    kokorosUp === null ? 'status status--unknown' : kokorosUp ? 'status status--up' : 'status status--down'

  return (
    <div className="container">
      <header className="header">
        <h1>yap-box</h1>
        <div className="header__right">
          <select
            className="voice-select"
            value={selectedVoice}
            onChange={(e) => setSelectedVoice(e.target.value)}
            aria-label="Voice"
          >
            {(voices.length > 0 ? voices : [selectedVoice]).map((v) => (
              <option key={v} value={v}>{v}</option>
            ))}
          </select>
          <div className={statusClass} title={statusLabel}>
            <span className="status__dot" />
            <span className="status__label">{statusLabel}</span>
          </div>
        </div>
      </header>
      <textarea
        value={text}
        onChange={(e) => setText(e.target.value)}
        placeholder="Paste text here or drag a file onto this window..."
        rows={12}
      />
      {speaking ? (
        <button onClick={handleStop} className="button--stop">Stop</button>
      ) : (
        <button onClick={handleYap} disabled={!text.trim() || kokorosUp === false}>
          Yap
        </button>
      )}
      {kokorosUp === false && (
        <div className="help">
          <p className="help__title">Kokoros isn't running.</p>
          <p className="help__body">
            Start it in a terminal: <code>koko openai</code>
          </p>
          <p className="help__body">
            Don't have it installed? See the{' '}
            <a href="https://github.com/lucasjinreal/Kokoros" target="_blank" rel="noreferrer">
              Kokoros setup guide
            </a>
            .
          </p>
        </div>
      )}
      {error && <p className="error">{error}</p>}
    </div>
  )
}

export default App
