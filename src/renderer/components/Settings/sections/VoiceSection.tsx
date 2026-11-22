// Settings → Voice: how this computer's microphone sounds in calls (voice
// rooms and DM calls alike).
//
// Noise removal (Sew 0.10.2), On by default: Sew's `standard`, which runs
// DeepFilterNet3 from the weights the app bundles and falls back to RNNoise.
// Off sends the full audio, with echo cancellation and gain control kept.
// The choice is this computer's (the voice prefs in this app's storage, like
// the devices and "join muted"); the native engine gets it when it attaches
// and at once mid-call. The web client has no native engine, so the switch
// says it applies to the desktop app.

import { Toggle } from '@/components/ui'
import { isWebClient } from '@/lib/is-web'
import { useVoice, voice } from '@/stores/voice'
import { SETTING_ROW_HINT, SettingsGroup, SettingSwitchRow } from '../controls/SettingControls'
import type { SettingEntry } from '../searchManifest'
import { PRODUCT_NAME } from '@/lib/brand'

export const VOICE_MANIFEST: SettingEntry[] = [
  {
    id: 'voice.noise-removal',
    section: 'voice',
    label: 'Noise removal',
    description: 'Remove background noise from your microphone in calls',
    keywords: ['voice', 'noise', 'suppression', 'background', 'microphone', 'mic', 'call', 'deepfilternet', 'rnnoise', 'audio'],
  },
]

export function VoiceSection(): React.JSX.Element {
  const noiseRemoval = useVoice((s) => s.prefs.noiseRemoval)
  const web = isWebClient()

  return (
    <div data-testid="voice-section" className="w-full max-w-[52rem] space-y-6 pb-6">
      <header className="min-w-0">
        <h2 className="type-title text-[var(--color-text-primary)]">Voice</h2>
        <p className={SETTING_ROW_HINT}>How your microphone sounds in calls from this computer.</p>
      </header>

      <SettingsGroup title="On this computer" card settingId="voice.noise-removal">
        <SettingSwitchRow
          control={
            <Toggle
              data-testid="voice-noise-removal"
              aria-label="Noise removal"
              checked={noiseRemoval}
              onChange={(on) => voice.setPrefs({ noiseRemoval: on })}
            />
          }
        >
          <div className="type-row text-[var(--color-text-primary)]">Noise removal</div>
          <div className={SETTING_ROW_HINT}>
            {noiseRemoval
              ? 'On: background noise like typing, fans and traffic is removed before others hear you.'
              : 'Off: others hear your full microphone, background noise included. Echo cancellation and volume levelling stay on.'}
            {web ? ` Applies to calls in the ${PRODUCT_NAME} desktop app.` : ' Changes take effect at once, even in a call.'}
          </div>
        </SettingSwitchRow>
      </SettingsGroup>
    </div>
  )
}
