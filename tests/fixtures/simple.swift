import Foundation

public struct Config {
    let debug: Bool
    let name: String
}

public protocol Configurable {
    func configure(with config: Config)
}

public class AppDelegate: Configurable {
    func configure(with config: Config) {
        print(config.name)
    }

    func launch() {
        configure(with: Config(debug: false, name: "app"))
    }
}

public enum Theme {
    case light
    case dark
}

public func defaultConfig() -> Config {
    return Config(debug: false, name: "default")
}
