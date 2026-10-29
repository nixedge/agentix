package us.disasm.agentix.device

import android.Manifest
import android.annotation.SuppressLint
import android.bluetooth.BluetoothManager
import android.bluetooth.le.BluetoothLeScanner
import android.bluetooth.le.ScanCallback
import android.bluetooth.le.ScanResult
import android.bluetooth.le.ScanSettings
import android.content.Context
import android.content.pm.PackageManager
import android.location.LocationManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import us.disasm.agentix.R
import us.disasm.agentix.core.PlaudAdvertisement

data class RecorderAdvertisement(
    val address: String,
    val name: String?,
    val rssi: Int,
    val projectCode: Int?,
)

class PlaudScanner(private val context: Context) {
    var devices by mutableStateOf<List<RecorderAdvertisement>>(emptyList())
        private set
    var scanning by mutableStateOf(false)
        private set
    var status by mutableStateOf(context.getString(R.string.scan_idle))
        private set

    private val handler = Handler(Looper.getMainLooper())
    private var scanner: BluetoothLeScanner? = null
    private val timeout = Runnable { stop() }

    val permissions: Array<String>
        get() = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            arrayOf(Manifest.permission.BLUETOOTH_SCAN, Manifest.permission.BLUETOOTH_CONNECT)
        } else {
            arrayOf(Manifest.permission.ACCESS_FINE_LOCATION)
        }

    fun hasPermissions(): Boolean = permissions.all {
        context.checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED
    }

    fun permissionDenied() {
        status = context.getString(R.string.scan_permissions)
    }

    private val callback = object : ScanCallback() {
        override fun onScanResult(callbackType: Int, result: ScanResult) {
            accept(result)
        }

        override fun onBatchScanResults(results: MutableList<ScanResult>) {
            results.forEach(::accept)
        }

        override fun onScanFailed(errorCode: Int) {
            handler.post {
                stop()
                status = context.getString(R.string.scan_failed, errorCode)
            }
        }
    }

    private fun accept(result: ScanResult) {
        val record = result.scanRecord ?: return
        val payload = record.getManufacturerSpecificData(PlaudAdvertisement.MANUFACTURER_ID)
        if (!PlaudAdvertisement.matches(record.deviceName, payload)) return
        val device = RecorderAdvertisement(
            result.device.address,
            record.deviceName,
            result.rssi,
            PlaudAdvertisement.projectCode(payload),
        )
        handler.post {
            if (scanning) {
                devices = (devices.filterNot { it.address == device.address } + device)
                    .sortedByDescending { it.rssi }
            }
        }
    }

    @SuppressLint("MissingPermission")
    fun start() {
        // Check permissions again at the platform boundary; Android can revoke them at runtime.
        if (!hasPermissions()) {
            permissionDenied()
            return
        }
        stop()
        devices = emptyList()
        if (!context.packageManager.hasSystemFeature(PackageManager.FEATURE_BLUETOOTH_LE)) {
            status = context.getString(R.string.scan_unavailable)
            return
        }
        try {
            val adapter = context.getSystemService(BluetoothManager::class.java)?.adapter
            if (adapter == null || !adapter.isEnabled) {
                status = context.getString(R.string.scan_disabled)
                return
            }
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
                val location = context.getSystemService(LocationManager::class.java)
                if (location == null || (!location.isProviderEnabled(LocationManager.GPS_PROVIDER) &&
                    !location.isProviderEnabled(LocationManager.NETWORK_PROVIDER))) {
                    status = context.getString(R.string.scan_location)
                    return
                }
            }
            val availableScanner = adapter.bluetoothLeScanner
            if (availableScanner == null) {
                status = context.getString(R.string.scan_unavailable)
                return
            }
            scanner = availableScanner
            scanning = true
            status = context.getString(R.string.scan_running)
            availableScanner.startScan(
                emptyList(),
                ScanSettings.Builder().setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY).build(),
                callback,
            )
            handler.postDelayed(timeout, 15_000)
        } catch (_: SecurityException) {
            stop()
            permissionDenied()
        } catch (_: IllegalStateException) {
            stop()
            status = context.getString(R.string.scan_disabled)
        }
    }

    @SuppressLint("MissingPermission")
    fun stop() {
        handler.removeCallbacks(timeout)
        val wasScanning = scanning
        scanning = false
        try {
            if (hasPermissions()) scanner?.stopScan(callback)
        } catch (_: SecurityException) {
            // Permission may be revoked between the check and stopScan.
        } catch (_: IllegalStateException) {
            // Bluetooth may be switched off while a scan is active.
        }
        scanner = null
        if (wasScanning) status = context.getString(R.string.scan_finished)
    }
}
