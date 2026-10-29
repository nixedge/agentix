package us.disasm.agentix.core.assistant

import org.junit.Assert.*
import org.junit.Test

class OwnerAuthenticationTest {
    @Test fun optionalCredentialOmitsHeaderAndConfiguredCredentialIsPreserved() {
        assertNull(OwnerAuthentication.authorization(""))
        val token = "synthetic-owner-token-0123456789abcdef"
        assertEquals("Bearer $token", OwnerAuthentication.authorization(token))
    }

    @Test fun malformedConfiguredCredentialsCannotSilentlyDisableAuthentication() {
        for (token in listOf("short", " ".repeat(32), "é".repeat(32), "x".repeat(31) + "\n", "x".repeat(513))) {
            assertThrows(IllegalArgumentException::class.java) { OwnerAuthentication.authorization(token) }
        }
    }
}
