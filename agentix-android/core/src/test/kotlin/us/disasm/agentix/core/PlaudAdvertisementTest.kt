package us.disasm.agentix.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PlaudAdvertisementTest {
    @Test
    fun readsUnsignedLittleEndianProjectCodeFromPayload() {
        assertEquals(888, PlaudAdvertisement.projectCode(byteArrayOf(0, 0x78, 0x03)))
        assertEquals(65535, PlaudAdvertisement.projectCode(byteArrayOf(0, -1, -1)))
    }

    @Test
    fun toleratesMissingAndTruncatedPayloads() {
        assertNull(PlaudAdvertisement.projectCode(null))
        for (size in 0..2) assertNull(PlaudAdvertisement.projectCode(ByteArray(size)))
        assertFalse(PlaudAdvertisement.matches(null, null))
        assertFalse(PlaudAdvertisement.matches("Other recorder", byteArrayOf(0, 1, 2)))
    }

    @Test
    fun discoversByNameOrKnownCodeWithoutClaimingModelCompatibility() {
        assertTrue(PlaudAdvertisement.matches("PLAUD_NOTE", null))
        assertTrue(PlaudAdvertisement.matches("Plaud1234", byteArrayOf(0)))
        assertTrue(PlaudAdvertisement.matches(null, byteArrayOf(0, 0x78, 0x03)))
    }
}
