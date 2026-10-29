package us.disasm.agentix.core.assistant

import org.junit.Assert.*
import org.junit.Test

class AssistantTurnTest {
    @Test fun instructionPreservesWordingAndOnlyFinalTextBecomesCommand() {
        val turn = AssistantTurn()
        val id = turn.begin()
        assertTrue(turn.partial(id, "add pine"))
        assertEquals(TurnPhase.LISTENING, turn.phase)
        assertFalse(turn.complete(id))
        val command = turn.recognize(id, "Add pineapple to my grocery list!")
        assertEquals("Add pineapple to my grocery list!", command?.text)
        assertEquals(TurnPhase.PROCESSING, turn.phase)
        assertFalse(turn.partial(id, "late partial"))
        assertNull(turn.recognize(id, "duplicate result"))
        assertTrue(turn.complete(id))
        assertFalse(turn.complete(id))
        assertFalse(turn.fail(id))
    }

    @Test fun cancelledRecognitionAndReplyCannotReviveTurn() {
        val turn = AssistantTurn()
        val cancelled = turn.begin()
        turn.cancel()
        assertFalse(turn.partial(cancelled, "late"))
        assertNull(turn.recognize(cancelled, "late"))
        assertFalse(turn.fail(cancelled))
        val current = turn.begin()
        assertNotEquals(cancelled, current)
        assertNotNull(turn.recognize(current, "new instruction"))
        assertFalse(turn.complete(cancelled))
        turn.cancel()
        assertFalse(turn.complete(current))
        assertEquals(TurnPhase.IDLE, turn.phase)
    }

    @Test fun blankOrFailedRecognitionCannotDispatch() {
        val turn = AssistantTurn()
        val id = turn.begin()
        assertNull(turn.recognize(id, " \n "))
        assertEquals(TurnPhase.ERROR, turn.phase)
        assertNull(turn.recognize(id, "late text"))
        val retry = turn.begin()
        assertTrue(turn.fail(retry))
        assertNull(turn.recognize(retry, "after error"))
        assertFalse(turn.complete(retry))
    }

    @Test(expected = IllegalArgumentException::class)
    fun blankTellIsRejectedAtBoundary() {
        TellCommand(" \t")
    }
}
