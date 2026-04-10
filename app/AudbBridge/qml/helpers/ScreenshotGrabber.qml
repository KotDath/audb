import QtQuick 2.0
import org.nemomobile.lipstick 0.1

QtObject {
    property bool completed: false
    property bool succeeded: false
    property string lastError: ""

    signal finished()

    function capture(path) {
        completed = false
        succeeded = false
        lastError = ""
        try {
            var settled = false
            var result = LipstickApi.takeScreenshot(path)
            if (!result) {
                completed = true
                succeeded = false
                lastError = "LipstickApi.takeScreenshot returned null"
                finished()
                return
            }

            result.finished.connect(function() {
                if (settled)
                    return
                settled = true
                completed = true
                succeeded = true
                lastError = ""
                finished()
            })

            result.error.connect(function() {
                if (settled)
                    return
                settled = true
                completed = true
                succeeded = false
                lastError = "LipstickApi reported capture error"
                finished()
            })
        } catch (error) {
            completed = true
            succeeded = false
            lastError = error.toString()
            finished()
        }
    }
}
