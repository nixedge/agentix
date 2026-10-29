package us.disasm.agentix.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class AssistantEndpointTest {
    @Test
    fun acceptsPrivateHostsPortsIpv6AndBasePaths() {
        for (url in listOf("https://assistant.lan:8765/api", "https://10.20.0.1", "https://[fd00::1]:8765/")) {
            assertEquals(url, AssistantEndpoint.normalize(" $url "))
        }
        assertEquals("https://assistant.lan/api/", AssistantEndpoint.normalize("https://assistant.lan/old/../api/"))
    }

    @Test
    fun rejectsPlaintextCredentialsAmbiguousComponentsAndInvalidPorts() {
        for (url in listOf(
            "", "assistant.lan", "http://assistant.lan", "file:///tmp/server",
            "https://", "https://user:secret@assistant.lan", "https://assistant.lan?token=secret",
            "https://assistant.lan#fragment", "https://assistant.lan:0", "https://assistant.lan:65536",
            "https://assistant.lan:abc", "https://bad host", "https://assistant.lan/%zz",
        )) {
            assertNull(url, AssistantEndpoint.normalize(url))
        }
    }
}
