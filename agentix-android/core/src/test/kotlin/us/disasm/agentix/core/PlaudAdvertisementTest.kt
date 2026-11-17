package us.disasm.agentix.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PlaudAdvertisementTest {
    private val advertisement = byteArrayOf(
        2, 0x78, 3, 4, 0x56, 0x5f, 0, 0, 5,
        0x88.toByte(), 0x80.toByte(), 0, 0, 1, 0, 0, 0, 0, 10, 0, 1, 1,
    )

    @Test
    fun decodesDocumentedAdvertisementHeaderWithoutPublishingSerial() {
        val report = PlaudAdvertisement.diagnostic(advertisement)
        assertTrue(report.contains("Advertised firmware: V0095 (numeric 95)"))
        assertTrue(report.contains("Advertised protocol: 10 (not authenticated)"))
        assertTrue(report.contains("Project/version prefix: 02 78 03 04 56 5f 00 00\n"))
        assertFalse(report.contains("88 80"))
        assertFalse(report.contains("8880000001"))
        assertTrue(PlaudAdvertisement.diagnostic(advertisement, 0x0046).contains("Advertised firmware: V0095"))
        assertTrue(PlaudAdvertisement.diagnostic(advertisement, 0x1234).contains("unsupported manufacturer"))
    }

    @Test
    fun keepsFirmwareAndProtocolDistinctForNewerAndOtherProjects() {
        val newer = advertisement.copyOf().also {
            it[4] = 'T'.code.toByte(); it[5] = 7; it[6] = 2; it[7] = 1
            it[18] = 20
        }
        val report = PlaudAdvertisement.diagnostic(newer)
        assertTrue(report.contains("Advertised firmware: T66055"))
        assertTrue(report.contains("Advertised protocol: 20"))
        val other = advertisement.copyOf().also { it[1] = 0x70; it[15] = 7 }
        assertTrue(PlaudAdvertisement.diagnostic(other).contains("Advertised protocol: 7"))
    }

    @Test
    fun truncationNeverGuessesMissingFirmwareOrProtocol() {
        for (size in advertisement.indices) {
            val report = PlaudAdvertisement.diagnostic(advertisement.copyOf(size))
            if (size < 14) assertTrue(report.contains("Advertised firmware: unavailable"))
            if (size < 20) assertFalse(report.contains("Advertised protocol: 10"))
            assertFalse(report.contains("88 80"))
        }
        assertTrue(PlaudAdvertisement.diagnostic(null).contains("not present"))
    }

    @Test
    fun unsupportedLengthsProjectsTypesAndOversizedDataAreRedacted() {
        val cases = listOf(
            advertisement.copyOf().also { it[0] = 1 },
            advertisement.copyOf().also { it[1] = 1 },
            advertisement.copyOf().also { it[3] = 0 },
            advertisement.copyOf().also { it[3] = 127 },
            advertisement.copyOf().also { it[4] = 10 },
            advertisement.copyOf().also { it[8] = -1 },
            advertisement.copyOf(513),
        )
        cases.forEach {
            val report = PlaudAdvertisement.diagnostic(it)
            assertTrue(report.contains("Advertised firmware: unavailable"))
            assertFalse(report.contains("Project/version prefix:"))
        }
    }

    @Test
    fun doesNotReadProtocolInsideLongSerialNumbers() {
        val raw = advertisement.copyOf(32).also { it[8] = 12 }
        val report = PlaudAdvertisement.diagnostic(raw)
        assertTrue(report.contains("Advertised firmware: V0095"))
        assertTrue(report.contains("Advertised protocol: unavailable"))
        assertFalse(report.contains("88 80"))
    }

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
