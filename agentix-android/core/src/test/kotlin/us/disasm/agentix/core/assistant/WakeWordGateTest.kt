package us.disasm.agentix.core.assistant

import org.junit.Test
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue

class WakeWordGateTest {
    @Test fun competingCaptureWaitsForActualRelease() {
        val gate = WakeWordGate()
        assertTrue(gate.reserve())
        var started = false
        val command = Any()
        gate.hold(command) { started = true }
        assertFalse(started)
        assertFalse(gate.reserve())
        gate.quiet()
        assertTrue(started)
        assertFalse(gate.reserve())
        gate.release(command)
        assertTrue(gate.reserve())
    }

    @Test fun cancellationRemovesLateCallback() {
        val gate = WakeWordGate()
        gate.reserve()
        var started = false
        val owner = Any()
        gate.hold(owner) { started = true }
        gate.release(owner)
        gate.quiet()
        assertFalse(started)
        assertTrue(gate.reserve())
    }

    @Test fun independentHoldsAndReplacementDoNotResumeEarly() {
        val gate = WakeWordGate()
        gate.reserve()
        val recognition = Any()
        val screen = Any()
        val calls = mutableListOf<Int>()
        gate.hold(screen) { calls += 1 }
        gate.hold(recognition) { calls += 2 }
        gate.hold(recognition) { calls += 3 }
        gate.quiet()
        assertEquals(listOf(1, 3), calls)
        gate.release(recognition)
        assertFalse(gate.reserve())
        gate.release(screen)
        assertTrue(gate.reserve())
    }

    @Test fun quietHoldIsImmediateAndStillBlocksCapture() {
        val gate = WakeWordGate()
        val owner = Any()
        var started = false
        gate.hold(owner) { started = true }
        assertTrue(started)
        assertFalse(gate.reserve())
        gate.release(owner)
        assertTrue(gate.reserve())
    }

    @Test fun staleDisabledAndRepeatedSignalsNeverLaunch() {
        val wake = WakeAdmission()
        val old = wake.begin()
        wake.invalidate()
        assertFalse(wake.accept(old, 0, true))
        val current = wake.begin()
        assertFalse(wake.accept(current, 0, false))
        assertTrue(wake.accept(current, 0, true))
        assertFalse(wake.accept(current, 5000, true))
        val next = wake.begin()
        assertFalse(wake.accept(next, 2999, true))
        assertTrue(wake.accept(next, 3000, true))
    }
}
