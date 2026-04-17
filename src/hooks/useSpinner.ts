import { useEffect, useState } from 'react'

// Braille spinner frames from gunnargray-dev/unicode-animations (MIT).
const FRAMES = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏']
const INTERVAL_MS = 80

export function useSpinner(active: boolean): string {
  const [index, setIndex] = useState(0)

  useEffect(() => {
    if (!active) {
      setIndex(0)
      return
    }
    const id = setInterval(() => {
      setIndex((i) => (i + 1) % FRAMES.length)
    }, INTERVAL_MS)
    return () => clearInterval(id)
  }, [active])

  return FRAMES[index]
}
