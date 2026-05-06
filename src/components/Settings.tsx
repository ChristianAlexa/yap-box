import { useEffect, useState } from 'react'
import pkg from '../../package.json'

type TabId = 'voice' | 'engine' | 'about'

interface SettingsProps {
  voices: string[]
  selectedVoice: string
  setSelectedVoice: (v: string) => void
  speaking: boolean
  previewing: boolean
  onPreview: () => void
  engineConnected: boolean
  onClose: () => void
}

const REPO_URL = 'https://github.com/ChristianAlexa/yap-box'

export function Settings(props: SettingsProps) {
  const [selected, setSelected] = useState<TabId>('voice')

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape') props.onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [props])

  const tabs: { id: TabId; label: string; icon: string }[] = [
    { id: 'voice', label: 'Voice', icon: '🎤' },
    { id: 'engine', label: 'Engine', icon: '🔌' },
    { id: 'about', label: 'About', icon: 'ℹ' },
  ]

  return (
    <div className="settings">
      <div className="settings__topbar">
        <button className="icon-button" onClick={props.onClose} aria-label="Back" title="Back">
          ←
        </button>
        <span className="settings__title">Settings</span>
      </div>
      <div className="settings__body">
        <nav className="settings__sidebar" aria-label="Settings categories">
          {tabs.map((t) => (
            <button
              key={t.id}
              className={'settings__tab' + (selected === t.id ? ' settings__tab--active' : '')}
              onClick={() => setSelected(t.id)}
            >
              <span className="settings__tab-icon" aria-hidden="true">
                {t.icon}
              </span>
              <span className="settings__tab-label">{t.label}</span>
            </button>
          ))}
        </nav>
        <div className="settings__pane">
          {selected === 'voice' && <VoicePane {...props} />}
          {selected === 'engine' && <EnginePane connected={props.engineConnected} />}
          {selected === 'about' && <AboutPane />}
        </div>
      </div>
    </div>
  )
}

function VoicePane({
  voices,
  selectedVoice,
  setSelectedVoice,
  speaking,
  previewing,
  onPreview,
}: SettingsProps) {
  return (
    <div className="pane">
      <h3 className="pane__heading">Voice</h3>
      <div className="pane__row">
        <select
          className="voice-select"
          value={selectedVoice}
          onChange={(e) => setSelectedVoice(e.target.value)}
          aria-label="Voice"
        >
          {(voices.length > 0 ? voices : [selectedVoice]).map((v) => (
            <option key={v} value={v}>
              {v}
            </option>
          ))}
        </select>
        <button
          className="preview-btn"
          onClick={onPreview}
          disabled={previewing || speaking}
          title={speaking ? 'Stop playback to preview' : 'Preview voice'}
          aria-label="Preview voice"
        >
          {previewing ? '…' : '▶'}
        </button>
      </div>
      <p className="pane__help">Voices are previewed with a short sample sentence.</p>
    </div>
  )
}

function EnginePane({ connected }: { connected: boolean }) {
  return (
    <div className="pane">
      <h3 className="pane__heading">Engine</h3>
      <div className={'status ' + (connected ? 'status--up' : 'status--down')}>
        <span className="status__dot" />
        <span className="status__label">
          {connected ? 'Kokoros connected' : 'Kokoros disconnected'}
        </span>
      </div>
      <p className="pane__help">The local TTS engine that synthesizes speech from your text.</p>
    </div>
  )
}

function AboutPane() {
  return (
    <div className="pane">
      <h3 className="pane__heading">yap-box</h3>
      <p className="pane__meta">Version {pkg.version}</p>
      <p className="pane__meta">© 2026 Christian Alexa</p>
      <ul className="pane__links">
        <li>
          <a href={REPO_URL} target="_blank" rel="noreferrer noopener">
            README
          </a>
        </li>
        <li>
          <a href={`${REPO_URL}/blob/main/PRIVACY.md`} target="_blank" rel="noreferrer noopener">
            Privacy
          </a>
        </li>
        <li>
          <a
            href={`${REPO_URL}/blob/main/THIRD_PARTY_LICENSES.md`}
            target="_blank"
            rel="noreferrer noopener"
          >
            Third-party licenses
          </a>
        </li>
      </ul>
    </div>
  )
}
