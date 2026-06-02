plugins {
    id("com.android.application")
    id("kotlin-android")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

val erpAndroidKeystore = System.getenv("ERP_ANDROID_KEYSTORE")
val erpAndroidKeystorePassword = System.getenv("ERP_ANDROID_KEYSTORE_PASSWORD")
val erpAndroidKeyAlias = System.getenv("ERP_ANDROID_KEY_ALIAS")
val erpAndroidKeyPassword = System.getenv("ERP_ANDROID_KEY_PASSWORD")
val hasErpReleaseSigning = listOf(
    erpAndroidKeystore,
    erpAndroidKeystorePassword,
    erpAndroidKeyAlias,
    erpAndroidKeyPassword,
).all { !it.isNullOrBlank() }

android {
    namespace = "uk.nam2.erp.erp_gui"
    compileSdk = flutter.compileSdkVersion
    ndkVersion = flutter.ndkVersion

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = JavaVersion.VERSION_17.toString()
    }

    defaultConfig {
        applicationId = "uk.nam2.erp.erp_gui"
        minSdk = flutter.minSdkVersion
        targetSdk = flutter.targetSdkVersion
        versionCode = flutter.versionCode
        versionName = flutter.versionName

    }

    packaging {
        jniLibs {
            useLegacyPackaging = true
            keepDebugSymbols += listOf("**/liberp_exec.so")
        }
    }

    signingConfigs {
        create("erpRelease") {
            if (hasErpReleaseSigning) {
                storeFile = file(erpAndroidKeystore!!)
                storePassword = erpAndroidKeystorePassword!!
                keyAlias = erpAndroidKeyAlias!!
                keyPassword = erpAndroidKeyPassword!!
            }
        }
    }

    buildTypes {
        debug {
            if (hasErpReleaseSigning) {
                signingConfig = signingConfigs.getByName("erpRelease")
            }
        }
        getByName("release") {
            signingConfig = if (hasErpReleaseSigning) {
                signingConfigs.getByName("erpRelease")
            } else {
                signingConfigs.getByName("debug")
            }
        }
    }
}

flutter {
    source = "../.."
}
